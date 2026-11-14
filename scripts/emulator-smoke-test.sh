#!/usr/bin/env bash
# Launch com.rkb.rust on a connected emulator/device and verify that it stays
# open, reaches RESUMED, and presents rendered frames.
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

# Suppress the one-time immersive-mode cling so screenshots show the app.
adb shell settings put secure immersive_mode_confirmations confirmed || true
adb logcat -c

log "launching ${ACTIVITY}"
LAUNCH_OUTPUT="$(adb shell am start -W -n "${ACTIVITY}")"
echo "${LAUNCH_OUTPUT}"
grep -q 'Status: ok' <<<"${LAUNCH_OUTPUT}" || fail "am start did not report Status: ok"
grep -q "Activity: ${ACTIVITY}" <<<"${LAUNCH_OUTPUT}" || fail "am start did not report ${ACTIVITY}"

sleep "${WAIT_SECONDS}"

adb exec-out screencap -p > "${OUT}/emulator-screen.png"
adb shell dumpsys activity activities > "${OUT}/emulator-activities.txt"
adb shell ps -A > "${OUT}/emulator-ps.txt" || true
adb shell pidof "${PKG}" > "${OUT}/emulator-pidof.txt" || true

log "waiting ${SETTLE_SECONDS}s more to confirm the app stays open"
sleep "${SETTLE_SECONDS}"

adb exec-out screencap -p > "${OUT}/emulator-screen-late.png"
adb shell dumpsys activity activities > "${OUT}/emulator-activities-late.txt"
adb logcat -d > "${OUT}/emulator-logcat.txt"

# Extract the app's PID from dumpsys rather than relying on `pidof`, which
# returns empty on some Android images even while the process is alive.
pid_from_dump() {
    sed -n 's/.*app=ProcessRecord{[0-9a-f]* \([0-9][0-9]*\):com\.rkb\.rust\/.*/\1/p' "$1" | head -n1
}

FIRST_PID="$(pid_from_dump "${OUT}/emulator-activities.txt")"
LATE_PID="$(pid_from_dump "${OUT}/emulator-activities-late.txt")"
log "pid from dumpsys: first=${FIRST_PID:-unknown} late=${LATE_PID:-unknown}"
log "pidof output: $(tr -d '\r\n' < "${OUT}/emulator-pidof.txt" | head -c 64)"

[ -n "${FIRST_PID}" ] || fail "dumpsys did not report a ProcessRecord for ${PKG}"
[ "${FIRST_PID}" = "${LATE_PID}" ] || fail "${PKG} was restarted (${FIRST_PID} -> ${LATE_PID})"
adb shell test -d "/proc/${LATE_PID}" || fail "/proc/${LATE_PID} disappeared"

for dump in emulator-activities.txt emulator-activities-late.txt; do
    grep -q "topResumedActivity=ActivityRecord{.*${PKG}/.MainActivity" "${OUT}/${dump}" \
        || fail "${dump}: ${PKG}/.MainActivity is not the top resumed activity"
    grep -q 'state=RESUMED' "${OUT}/${dump}" \
        || fail "${dump}: activity is not RESUMED"
    grep -q 'reportedDrawn=true' "${OUT}/${dump}" \
        || fail "${dump}: activity never reported a drawn window"
done

grep -q 'RKB RUST native version' "${OUT}/emulator-logcat.txt" \
    || fail "native library never initialized"
grep -q 'native engine created' "${OUT}/emulator-logcat.txt" \
    || fail "native engine was never created"
grep -q 'first frame presented' "${OUT}/emulator-logcat.txt" \
    || fail "no frame was presented"

if grep -qE "${CRASH_PATTERN}" "${OUT}/emulator-logcat.txt"; then
    grep -n -B2 -A60 -E "${CRASH_PATTERN}" "${OUT}/emulator-logcat.txt"
    fail "runtime crash detected in logcat"
fi

if grep -qE 'ANR in com\.rkb\.rust|am_anr.*com\.rkb\.rust' "${OUT}/emulator-logcat.txt"; then
    grep -n -B2 -A40 -E 'ANR in com\.rkb\.rust|am_anr.*com\.rkb\.rust' "${OUT}/emulator-logcat.txt"
    fail "ANR detected for ${PKG}"
fi

log "startup summary"
grep -E 'RKB RUST native version|native engine created|first frame presented' \
    "${OUT}/emulator-logcat.txt" || true
log "PASS: ${PKG} stayed open, rendered frames, and logged no crash"
