#!/usr/bin/env bash
# Full local verification: format, lint, tests, release builds, demo render,
# and PAK archive round-trip.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

OUT="${ROOT}/artifacts/verify"
rm -rf "${OUT}"
mkdir -p "${OUT}"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
cargo run --locked -p rkb-demo --release -- \
    --frames 120 --width 640 --height 360 --out "${OUT}/demo.ppm"
./scripts/pack-assets.sh crates/rkb-sandbox/assets "${OUT}/assets.pak"
cargo run --locked -p rkb-pak --release -- verify "${OUT}/assets.pak"

echo "verification complete: ${OUT}"
