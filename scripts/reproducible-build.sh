#!/usr/bin/env bash
# Rebuild in a digest-pinned compiler + linker environment; never execute the binaries.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
image='mirror.gcr.io/library/rust@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b'
mode=compare
offline=false
for arg in "$@"; do
  case "$arg" in
    --update-bundled) mode=update ;;
    --offline) offline=true ;;
    *) printf 'Usage: %s [--update-bundled] [--offline]\n' "$0" >&2; exit 2 ;;
  esac
done
[[ $(uname -m) == x86_64 ]] || { echo 'Requires x86_64.' >&2; exit 1; }
if [[ $mode == compare && -n $(git -C "$root" status --porcelain --untracked-files=normal) ]]; then
  echo 'Comparison evidence requires a clean Git checkout. Commit reviewed changes first.' >&2
  exit 1
fi
uid=${SUDO_UID:-$(id -u)}
gid=${SUDO_GID:-$(id -g)}
cache=${NATIVE_CARGO_REGISTRY:-$HOME/.cache/omaclipsync-native/registry}
mkdir -p "$cache" "$root/dist"
build=$(mktemp -d)
trap 'rm -rf -- "$build"' EXIT
mkdir "$build/cargo"
chown "$uid:$gid" "$build"
chown "$uid:$gid" "$build/cargo"
registry_mount="type=bind,src=$cache,dst=/cargo/registry"
if $offline; then registry_mount+=',readonly'; fi
run=(docker run --rm --platform linux/amd64 --cap-drop ALL --security-opt no-new-privileges
  --user "$uid:$gid" --mount "type=bind,src=$root,dst=/src,readonly"
  --mount "type=bind,src=$build,dst=/build" --mount "type=bind,src=$build/cargo,dst=/cargo"
  --mount "$registry_mount"
  --workdir /src --env CARGO_HOME=/cargo --env CARGO_TARGET_DIR=/build
  --env CARGO_INCREMENTAL=0 --env SOURCE_DATE_EPOCH=0 --env TZ=UTC --env LC_ALL=C
  --env 'RUSTFLAGS=--remap-path-prefix=/src=/omaclipsync')
if ! $offline; then
  "${run[@]}" "$image" cargo fetch --manifest-path core/Cargo.toml --locked
fi
"${run[@]}" --network none "$image" sh -ec '
  rustc -Vv
  cargo -V
  cc --version | head -1
  ld --version | head -1
  cargo build --manifest-path core/Cargo.toml --locked --offline --release --bins
' > "$root/dist/BUILDINFO.txt" 2>&1 || { cat "$root/dist/BUILDINFO.txt" >&2; exit 1; }
out="$root/dist/native"
mkdir -p "$out"
for name in syncclipd syncclipctl; do
  install -m 0755 "$build/release/$name" "$out/$name"
  if [[ $mode == update ]]; then
    install -m 0755 "$out/$name" "$root/bin/$name"
  fi
  cmp -- "$root/bin/$name" "$out/$name"
done
{
  printf 'Source commit: %s\nBuilder image: %s\n' "$(git -C "$root" rev-parse HEAD)" "$image"
  printf 'Build mode: %s\n' "$mode"
  printf 'Working tree changes (empty for a release verification):\n'
  git -C "$root" status --porcelain --untracked-files=normal
  cat "$root/dist/BUILDINFO.txt"
  if [[ $mode == compare ]]; then
    printf '\nBoth committed binaries match the independent source build byte for byte.\n'
  else
    printf '\nBundled files updated. Commit and run comparison mode to establish verification.\n'
  fi
  (cd "$root" && sha256sum bin/syncclipd bin/syncclipctl core/Cargo.lock)
} > "$root/dist/BUILD-EVIDENCE.txt"
chown -R "$uid:$gid" "$out" "$root/dist/BUILDINFO.txt" "$root/dist/BUILD-EVIDENCE.txt"
cat "$root/dist/BUILD-EVIDENCE.txt"
