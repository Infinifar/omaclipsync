#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
source_root=${RUSTCLIPSYNC_CORE_SOURCE:-"$root/../../rustclipsync-core"}
exec python3 "$source_root/tools/export_source.py" "$root/core/vendor/rustclipsync-core" "$@"
