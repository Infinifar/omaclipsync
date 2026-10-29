#!/usr/bin/env bash
set -euo pipefail

plugin_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
shell_dir="${OMARCHY_PATH:-/usr/share/omarchy}/shell"
if [[ -n ${QMLLINT:-} ]]; then
  lint_bin=$QMLLINT
elif command -v qmllint >/dev/null 2>&1; then
  lint_bin=$(command -v qmllint)
else
  lint_bin=/usr/lib/qt6/bin/qmllint
fi
[[ -x $lint_bin ]] || { printf 'qmllint not found: %s\n' "$lint_bin" >&2; exit 1; }
command -v jq >/dev/null || { printf '%s\n' 'jq is required to report qmllint diagnostics.' >&2; exit 1; }
[[ -f $shell_dir/Commons/qmldir && -f $shell_dir/Ui/qmldir ]] || {
  printf 'Omarchy QML modules not found: %s\n' "$shell_dir" >&2
  exit 1
}
shell_dir=$(cd -- "$shell_dir" && pwd -P)

# Quickshell resolves qs.* from the shell directory; qmllint expects a qs/ parent.
# Keep the temporary symlink outside the distributable plugin directory.
lint_tmp=$(mktemp -d)
trap 'rm -r -- "$lint_tmp"' EXIT
ln -s -- "$shell_dir" "$lint_tmp/qs"
status=0
"$lint_bin" -W 0 --json "$lint_tmp/results.json" -I "$lint_tmp" \
  "$plugin_dir/Service.qml" "$plugin_dir/BarWidget.qml" "$plugin_dir/Panel.qml" || status=$?
if [[ -s $lint_tmp/results.json ]]; then
  jq -r '.files[] | .filename as $file | .warnings[] | "\($file):\(.line):\(.column): \(.type) [\(.id)] \(.message)"' "$lint_tmp/results.json"
fi
if (( status != 0 )); then
  printf 'qmllint failed (exit %s).\n' "$status" >&2
  exit "$status"
fi
printf '%s\n' 'qmllint passed with no warnings.'
