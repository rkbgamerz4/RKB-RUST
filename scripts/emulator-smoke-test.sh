#!/usr/bin/env bash
# Launch com.rkb.rust on a connected emulator/device and verify two startup
# paths:
#
#   1. Fresh install (no save): the client must generate deterministic terrain
#      itself, persist it, stream chunks, and render block geometry.
#   2. Supplied save: a synthetic, legally unencumbered 1.21 save must be read
#      through the real save reader and rendered.
#
# Used by .github/workflows/android-emulator.yml. The workflow runner executes
# each line of its `script` block in a separate shell, so all multi-line logic
# must live here.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APK="${ROOT}/android/app/build/outputs/apk/debug/app-debug.apk"
OUT="${RKB_EMULATOR_OUT:-${ROOT}}"
PKG="com.rkb.rust"
ACTIVITY="${PKG}/.MainActivity"
WAIT_SECONDS="${RKB_EMULATOR_WAIT:-15}"
SETTLE_SECONDS="${RKB_EMULATOR_SETTLE:-5}"
CRASH_PATTERN='FATAL EXCEPTION|Fatal signal|Abort message|UnsatisfiedLinkError'

log() {
    echo "[emulator-smoke] $*"
}

fail() {
    echo "[emulator-smoke] error: $*" >&2
    exit 1
}

mkdir -p "${OUT}"

log "installing ${APK}"
adb install -r "${APK}"

# Start from a genuinely empty install so the default generated world is what
# the app creates, not a world left over from an earlier script run.
log "clearing app data for a fresh-install startup"
adb shell pm clear "${PKG}" >/dev/null || true

# Suppress the one-time immersive-mode cling so screenshots show the app.
adb shell settings put secure immersive_mode_confirmations confirmed || true

launch_and_wait() {
    adb logcat -c
    log "launching ${ACTIVITY}"
    local launch_output
    launch_output="$(adb shell am start -W -n "${ACTIVITY}")"
    echo "${launch_output}"
    grep -q 'Status: ok' <<<"${launch_output}" || fail "am start did not report Status: ok"
    grep -q "Activity: ${ACTIVITY}" <<<"${launch_output}" \
        || fail "am start did not report ${ACTIVITY}"
    sleep "${WAIT_SECONDS}"
}

stop_app() {
    adb shell am force-stop "${PKG}" || true
    sleep 1
}

capture_activity() {
    local suffix="$1"
    adb exec-out screencap -p > "${OUT}/emulator-screen${suffix}.png"
    adb shell dumpsys activity activities > "${OUT}/emulator-activities${suffix}.txt"
    adb shell ps -A > "${OUT}/emulator-ps${suffix}.txt" || true
    adb shell pidof "${PKG}" > "${OUT}/emulator-pidof${suffix}.txt" || true
}

# Extract the app's PID from dumpsys rather than relying on `pidof`, which
# returns empty on some Android images even while the process is alive.
pid_from_dump() {
    sed -n 's/.*app=ProcessRecord{[0-9a-f]* \([0-9][0-9]*\):com\.rkb\.rust\/.*/\1/p' "$1" | head -n1
}

assert_process_alive() {
    local suffix="$1"
    local first_pid late_pid

    first_pid="$(pid_from_dump "${OUT}/emulator-activities${suffix}.txt")"
    late_pid="$(pid_from_dump "${OUT}/emulator-activities${suffix}-late.txt")"
    log "pid from dumpsys (${suffix:-save}): first=${first_pid:-unknown} late=${late_pid:-unknown}"
    [ -n "${first_pid}" ] || fail "${suffix:-save}: dumpsys did not report a ProcessRecord for ${PKG}"
    [ "${first_pid}" = "${late_pid}" ] \
        || fail "${suffix:-save}: ${PKG} was restarted (${first_pid} -> ${late_pid})"
    adb shell test -d "/proc/${late_pid}" || fail "${suffix:-save}: /proc/${late_pid} disappeared"

    for dump in "emulator-activities${suffix}.txt" "emulator-activities${suffix}-late.txt"; do
        grep -q "topResumedActivity=ActivityRecord{.*${PKG}/.MainActivity" "${OUT}/${dump}" \
            || fail "${dump}: ${PKG}/.MainActivity is not the top resumed activity"
        grep -q 'state=RESUMED' "${OUT}/${dump}" \
            || fail "${dump}: activity is not RESUMED"
        grep -q 'reportedDrawn=true' "${OUT}/${dump}" \
            || fail "${dump}: activity never reported a drawn window"
    done
}

# The stats line is
#
#   world=loaded kind=generated chunks=49/81 visible=18 faces=28197 tris=10294 ...
#
# Parse the last complete line instead of grepping for adjacent fields: the
# fields are not adjacent (`kind=` sits between `world=` and `chunks=`), which
# made an earlier `world=loaded chunks=` pattern fail even when chunks rendered.
assert_common_log() {
    local logfile="$1"
    grep -q 'RKB RUST native version' "${logfile}" \
        || fail "${logfile}: native library never initialized"
    grep -q 'native engine created' "${logfile}" \
        || fail "${logfile}: native engine was never created"
    grep -q 'first frame presented' "${logfile}" \
        || fail "${logfile}: no frame was presented"

    local stats loaded generated faces tris
    stats="$(grep -oE 'world=loaded kind=[a-z]+ chunks=[0-9]+/[0-9]+ visible=[0-9]+ faces=[0-9]+ tris=[0-9]+' \
        "${logfile}" | tail -n1)"
    [ -n "${stats}" ] || fail "${logfile}: no world stats line was logged"
    loaded="$(sed -E 's/.*chunks=([0-9]+)\/[0-9]+.*/\1/' <<<"${stats}")"
    generated="$(sed -E 's/.*chunks=[0-9]+\/([0-9]+) .*/\1/' <<<"${stats}")"
    faces="$(sed -E 's/.*faces=([0-9]+) .*/\1/' <<<"${stats}")"
    tris="$(sed -E 's/.*tris=([0-9]+).*/\1/' <<<"${stats}")"
    log "world stats (${logfile##*/}): chunks=${loaded}/${generated} faces=${faces} tris=${tris}"

    [ "${loaded}" -ge 16 ] \
        || fail "${logfile}: only ${loaded} chunks were streamed (expected at least 16)"
    [ "${generated}" -ge "${loaded}" ] \
        || fail "${logfile}: generated ${generated} chunks but loaded ${loaded}"
    [ "${faces}" -ge 100 ] \
        || fail "${logfile}: only ${faces} block faces were meshed"
    [ "${tris}" -ge 100 ] \
        || fail "${logfile}: only ${tris} block triangles were rendered"
    if grep -q 'balls=' "${logfile}"; then
        fail "${logfile}: the old sandbox is still running"
    fi
}

# ---------------------------------------------------------------------------
# Scenario 1: fresh install with no save -> generated terrain.
# ---------------------------------------------------------------------------

launch_and_wait
capture_activity "-generated"
sleep "${SETTLE_SECONDS}"
capture_activity "-generated-late"
adb logcat -d > "${OUT}/emulator-logcat-generated.txt"

assert_process_alive "-generated"
GENERATED_LOG="${OUT}/emulator-logcat-generated.txt"
assert_common_log "${GENERATED_LOG}"
grep -q 'kind=generated' "${GENERATED_LOG}" \
    || fail "fresh install did not start a generated world"
grep -q 'Generated world seed' "${GENERATED_LOG}" \
    || fail "generated world seed was not reported"
adb shell "run-as ${PKG} test -f files/minecraft/world/level.dat" \
    || adb shell "run-as ${PKG} test -f /sdcard/Android/data/${PKG}/files/minecraft/world/level.dat" \
    || fail "generated world did not persist level.dat"
SESSION_FOUND=false
for SESSION_PATH in \
    "files/minecraft/world/rkb-session.json" \
    "/sdcard/Android/data/${PKG}/files/minecraft/world/rkb-session.json"; do
    if adb shell "run-as ${PKG} test -f '${SESSION_PATH}'"; then
        SESSION_FOUND=true
        break
    fi
done
[ "${SESSION_FOUND}" = true ] || fail "generated world did not persist rkb-session.json"

log "generated-world startup passed"
stop_app

# ---------------------------------------------------------------------------
# Scenario 2: supplied 1.21 save -> real save reader.
# ---------------------------------------------------------------------------

log "generating synthetic 1.21 save"
rm -rf "${OUT}/fixture-world"
(
    cd "${ROOT}"
    cargo run --quiet --locked -p rkb-minecraft --bin rkb-fixture -- "${OUT}/fixture-world"
)
adb shell rm -rf /data/local/tmp/rkb-fixture-world
adb push "${OUT}/fixture-world" /data/local/tmp/rkb-fixture-world
# Remove both world locations so the fixture is unambiguously selected.
adb shell "run-as ${PKG} sh -c 'rm -rf files/minecraft/world /sdcard/Android/data/${PKG}/files/minecraft/world'"
adb shell "run-as ${PKG} sh -c 'mkdir -p files/minecraft && cp -r /data/local/tmp/rkb-fixture-world files/minecraft/world'"
adb shell "run-as ${PKG} test -f files/minecraft/world/level.dat" \
    || fail "fixture save was not copied into app storage"

launch_and_wait
capture_activity ""
sleep "${SETTLE_SECONDS}"
capture_activity "-late"
adb logcat -d > "${OUT}/emulator-logcat.txt"

assert_process_alive ""
SAVE_LOG="${OUT}/emulator-logcat.txt"
assert_common_log "${SAVE_LOG}"
grep -q 'kind=save' "${SAVE_LOG}" \
    || fail "supplied save was not opened as a save world"
grep -q 'RKB Demo Island' "${SAVE_LOG}" \
    || fail "the loaded save was not identified"

if grep -qE "${CRASH_PATTERN}" "${GENERATED_LOG}" "${SAVE_LOG}"; then
    grep -n -B2 -A60 -E "${CRASH_PATTERN}" "${GENERATED_LOG}" "${SAVE_LOG}"
    fail "runtime crash detected in logcat"
fi

if grep -qE 'ANR in com\.rkb\.rust|am_anr.*com\.rkb\.rust' "${GENERATED_LOG}" "${SAVE_LOG}"; then
    grep -n -B2 -A40 -E 'ANR in com\.rkb\.rust|am_anr.*com\.rkb\.rust' \
        "${GENERATED_LOG}" "${SAVE_LOG}"
    fail "ANR detected for ${PKG}"
fi

log "startup summary (generated)"
grep -E 'RKB RUST native version|native engine created|first frame presented|minecraft client:|world=loaded kind=' \
    "${GENERATED_LOG}" || true
log "startup summary (save)"
grep -E 'RKB RUST native version|native engine created|first frame presented|minecraft client:|world=loaded kind=' \
    "${SAVE_LOG}" || true
log "PASS: ${PKG} generated terrain on a fresh install, loaded a supplied 1.21 save, rendered block geometry, and logged no crash"
