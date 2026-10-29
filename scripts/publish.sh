#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
: "${CI_COMMIT_TAG:?tag required}" "${CI_API_V4_URL:?API URL required}" "${CI_PROJECT_ID:?project ID required}" "${CI_JOB_TOKEN:?job token required}"
version=$(jq -er '.version' "$root/manifest.json")
[[ ${CI_COMMIT_TAG#v} == "$version" ]] || { printf '%s\n' 'Tag must match manifest version.' >&2; exit 1; }
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.-]+)?$ ]] || exit 1
[[ $CI_JOB_TOKEN =~ ^[A-Za-z0-9._-]+$ ]] || { printf '%s\n' 'Unexpected job token format.' >&2; exit 1; }
base="$CI_API_V4_URL/projects/$CI_PROJECT_ID/packages/generic/omaclipsync/$version"
tarball="omaclipsync-plugin-$version.tar.gz"
for file in syncclipd-linux-x86_64 "$tarball"; do
  [[ -f "$root/dist/$file" ]] || { printf 'Missing artifact: %s\n' "$file" >&2; exit 1; }
  printf 'header = "JOB-TOKEN: %s"\n' "$CI_JOB_TOKEN" | curl --config - --fail --silent --show-error --max-time 120 --upload-file "$root/dist/$file" "$base/$file" --output /dev/null
done
body=$(mktemp)
trap 'rm -f -- "$body"' EXIT
jq -n --arg tag "$CI_COMMIT_TAG" --arg bin "$base/syncclipd-linux-x86_64" --arg tar "$base/$tarball" \
  '{name:$tag,tag_name:$tag,assets:{links:[{name:"syncclipd-linux-x86_64",url:$bin},{name:"Plugin archive",url:$tar}]}}' > "$body"
printf 'header = "JOB-TOKEN: %s"\n' "$CI_JOB_TOKEN" | curl --config - --fail --silent --show-error --max-time 30 --request POST --header 'Content-Type: application/json' --data-binary "@$body" "$CI_API_V4_URL/projects/$CI_PROJECT_ID/releases" --output /dev/null
printf 'Published release %s\n' "$CI_COMMIT_TAG"
