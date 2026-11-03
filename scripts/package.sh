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
elif [[ ${1:-} == --use-verified && $# == 1 ]]; then
  binary="$root/dist/native/syncclipd"
  client="$root/dist/native/syncclipctl"
  cmp -- "$root/bin/syncclipd" "$binary"
  cmp -- "$root/bin/syncclipctl" "$client"
  [[ -s "$root/dist/BUILD-EVIDENCE.txt" ]] || { echo 'Run reproducible-build.sh first.' >&2; exit 1; }
  grep -Fxq "Source commit: $(git -C "$root" rev-parse HEAD)" "$root/dist/BUILD-EVIDENCE.txt"
  grep -Fxq 'Build mode: compare' "$root/dist/BUILD-EVIDENCE.txt"
  [[ -z $(git -C "$root" status --porcelain --untracked-files=normal) ]] || { echo 'Verified packaging requires a clean checkout.' >&2; exit 1; }
elif [[ $# == 0 ]]; then
  cargo build --manifest-path "$root/core/Cargo.toml" --locked --release --bins
  build_dir=${CARGO_TARGET_DIR:-"$root/core/target"}
  binary="$build_dir/release/syncclipd"
  client="$build_dir/release/syncclipctl"
else
  printf '%s\n' 'Usage: package.sh [--use-bundled|--use-verified]' >&2
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
cp "$root/manifest.json" "$root/Service.qml" "$root/BarWidget.qml" "$root/Panel.qml" "$root/Settings.js" "$root/Inbox.js" "$root/README.md" "$root/README-ZH.md" "$root/CHANGELOG.md" "$root/preview.png" "$root/ACCEPTANCE.md" "$root/NATIVE-REVIEW.md" "$root/REPRODUCIBLE-BUILD.md" "$root/LICENSE" "$root/THIRD_PARTY_NOTICES.md" "$stage/"
cp -R "$root/assets" "$stage/assets"
cp -R "$root/third-party-licenses" "$stage/third-party-licenses"
cp -R "$root/i18n" "$stage/i18n"
cp -R "$root/core/src" "$root/core/tests" "$root/core/Cargo.toml" "$root/core/Cargo.lock" "$stage/core/"
cp -R "$root/core/vendor" "$stage/core/vendor"
cp "$root/scripts/package.sh" "$root/scripts/qmllint.sh" "$root/scripts/reproducible-build.sh" "$stage/scripts/"
cp "$root/tests/settings.test.js" "$root/tests/settings.gjs" "$root/tests/inbox.test.js" "$root/tests/service.test.js" "$stage/tests/"
if [[ ${1:-} == --use-verified ]]; then
  cp "$root/dist/BUILD-EVIDENCE.txt" "$stage/"
fi
install -m 0755 "$binary" "$root/dist/syncclipd-linux-x86_64"
install -m 0755 "$client" "$root/dist/syncclipctl-linux-x86_64"
tar --sort=name --mtime='UTC 1970-01-01' --owner=0 --group=0 --numeric-owner -C "$stage" -czf "$root/dist/omaclipsync-plugin-$version.tar.gz" .
printf 'Created %s and %s\n' "$root/dist/syncclipd-linux-x86_64" "$root/dist/omaclipsync-plugin-$version.tar.gz"
