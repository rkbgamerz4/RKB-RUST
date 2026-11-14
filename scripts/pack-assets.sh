#!/usr/bin/env bash
# Build a deterministic RKBPAK1 archive from a directory.
# Usage: scripts/pack-assets.sh <input-dir> <output.pak> [--force]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -lt 2 ]]; then
    echo "usage: $0 <input-dir> <output.pak> [--force]" >&2
    exit 2
fi

cd "${ROOT}"
exec cargo run --quiet --locked -p rkb-pak -- pack "$@"
