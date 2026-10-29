#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
[[ $(uname -m) == x86_64 ]] || { printf '%s\n' 'Only x86_64 packages are supported.' >&2; exit 1; }
version=$(jq -er '.version' "$root/manifest.json")
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.-]+)?$ ]] || { printf '%s\n' 'Invalid version.' >&2; exit 1; }
if [[ -n $(find "$root" -path "$root/core/target" -prune -o -path "$root/target" -prune -o -path "$root/dist" -prune -o -name .git -prune -o -type l -print -quit) ]]; then
  printf '%s\n' 'Source directory contains a symbolic link.' >&2; exit 1
fi
if [[ ${1:-} == --use-bundled && $# == 1 ]]; then
  binary="$root/bin/syncclipd"
  client="$root/bin/syncclipctl"
elif [[ $# == 0 ]]; then
  cargo build --manifest-path "$root/core/Cargo.toml" --locked --release --bins
  build_dir=${CARGO_TARGET_DIR:-"$root/core/target"}
  binary="$build_dir/release/syncclipd"
  client="$build_dir/release/syncclipctl"
else
  printf '%s\n' 'Usage: package.sh [--use-bundled]' >&2
  exit 1
fi
[[ -f $binary ]] || { printf '%s\n' 'Built daemon not found.' >&2; exit 1; }
[[ -f $client ]] || { printf '%s\n' 'Built client not found.' >&2; exit 1; }
[[ $(stat -c %s "$binary") -lt 15728640 ]] || { printf '%s\n' 'Daemon exceeds 15 MiB.' >&2; exit 1; }
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/bin" "$stage/core" "$stage/scripts" "$stage/tests" "$root/dist"
install -m 0755 "$binary" "$stage/bin/syncclipd"
install -m 0755 "$client" "$stage/bin/syncclipctl"
cp "$root/manifest.json" "$root/Service.qml" "$root/BarWidget.qml" "$root/Panel.qml" "$root/Settings.js" "$root/README.md" "$root/README-ZH.md" "$root/preview.png" "$root/ACCEPTANCE.md" "$root/LICENSE" "$root/THIRD_PARTY_NOTICES.md" "$stage/"
cp -R "$root/assets" "$stage/assets"
cp -R "$root/third-party-licenses" "$stage/third-party-licenses"
cp -R "$root/i18n" "$stage/i18n"
cp -R "$root/core/src" "$root/core/tests" "$root/core/examples" "$root/core/Cargo.toml" "$root/core/Cargo.lock" "$stage/core/"
cp "$root/scripts/package.sh" "$stage/scripts/"
cp "$root/tests/settings.test.js" "$root/tests/settings.gjs" "$stage/tests/"
install -m 0755 "$binary" "$root/dist/syncclipd-linux-x86_64"
tar --sort=name --mtime='UTC 1970-01-01' --owner=0 --group=0 --numeric-owner -C "$stage" -czf "$root/dist/omaclipsync-plugin-$version.tar.gz" .
printf 'Created %s and %s\n' "$root/dist/syncclipd-linux-x86_64" "$root/dist/omaclipsync-plugin-$version.tar.gz"
