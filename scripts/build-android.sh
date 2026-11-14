#!/usr/bin/env bash
# Cross-compile the RKB RUST Android library for arm64-v8a.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
API="${RKB_ANDROID_API:-24}"
TARGET="${RKB_TARGET:-aarch64-linux-android}"
ABI="${RKB_ABI:-arm64-v8a}"
PROFILE="${RKB_PROFILE:-release}"

find_ndk() {
    if [[ -n "${ANDROID_NDK_HOME:-}" && -d "${ANDROID_NDK_HOME}" ]]; then
        echo "${ANDROID_NDK_HOME}"
        return
    fi
    if [[ -n "${ANDROID_NDK_LATEST_HOME:-}" && -d "${ANDROID_NDK_LATEST_HOME}" ]]; then
        echo "${ANDROID_NDK_LATEST_HOME}"
        return
    fi
    local sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
    if [[ -n "${sdk}" && -d "${sdk}/ndk" ]]; then
        find "${sdk}/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1
        return
    fi
    return 1
}

NDK="$(find_ndk || true)"
if [[ -z "${NDK}" ]]; then
    echo "error: Android NDK not found. Set ANDROID_NDK_HOME or install it with sdkmanager." >&2
    exit 1
fi

HOST_TAG="linux-x86_64"
case "$(uname -s)" in
    Darwin) HOST_TAG="darwin-x86_64" ;;
esac
TOOLCHAIN="${NDK}/toolchains/llvm/prebuilt/${HOST_TAG}/bin"
LINKER="${TOOLCHAIN}/${TARGET}${API}-clang"
if [[ ! -x "${LINKER}" ]]; then
    echo "error: linker not found at ${LINKER}" >&2
    exit 1
fi

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="${LINKER}"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="${LINKER}"

cd "${ROOT}"
rustup target add "${TARGET}" >/dev/null 2>&1 || true
if [[ "${PROFILE}" == "release" ]]; then
    cargo build --release --target "${TARGET}" -p rkb-android
    BUILT="target/${TARGET}/release/librkb_android.so"
else
    cargo build --target "${TARGET}" -p rkb-android
    BUILT="target/${TARGET}/debug/librkb_android.so"
fi

DEST="android/app/src/main/jniLibs/${ABI}"
mkdir -p "${DEST}"
cp "${BUILT}" "${DEST}/librkb_android.so"
"${TOOLCHAIN}/llvm-strip" --strip-unneeded "${DEST}/librkb_android.so" 2>/dev/null || true
echo "copied ${BUILT} -> ${DEST}/librkb_android.so"
file "${DEST}/librkb_android.so" 2>/dev/null || true
