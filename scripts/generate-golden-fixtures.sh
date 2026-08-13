#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
OLD_ARCHIVE_BIN=${OLD_ARCHIVE_BIN:?set OLD_ARCHIVE_BIN to the historical v1.9.6 hig binary}
OLD_REPOSITORY_BIN=${OLD_REPOSITORY_BIN:?set OLD_REPOSITORY_BIN to the pre-reference v1.10.0 hig binary}
ARCHIVE_ROOT="$ROOT/fixtures/archives/v1.9.6"
REPOSITORY_ROOT="$ROOT/fixtures/repositories/v1.10.0-direct-head"
PASSWORD='hig-public-fixture-v1'

if [[ -e "$ARCHIVE_ROOT" || -e "$REPOSITORY_ROOT" ]]; then
  echo "golden fixture destination already exists; immutable fixtures are never overwritten" >&2
  exit 1
fi

mkdir -p "$ARCHIVE_ROOT/source/src/nested" "$REPOSITORY_ROOT/workspace/src"
printf 'HIG historical fixture\n' > "$ARCHIVE_ROOT/source/README.txt"
printf 'fn historical_fixture() -> u8 { 7 }\n' > "$ARCHIVE_ROOT/source/src/nested/lib.rs"
: > "$ARCHIVE_ROOT/source/empty.bin"

archive_cache=$(mktemp -d "${TMPDIR:-/tmp}/hig-golden-cache.XXXXXX")
trap 'rm -rf "$archive_cache"' EXIT

"$OLD_ARCHIVE_BIN" pack "$ARCHIVE_ROOT/source" \
  --output "$ARCHIVE_ROOT/higv1-password.hig" --cache-dir "$archive_cache" \
  --format higv1 --password "$PASSWORD" --daemon off --project off --json >/dev/null
"$OLD_ARCHIVE_BIN" pack "$ARCHIVE_ROOT/source" \
  --output "$ARCHIVE_ROOT/higv2-legacy-password.hig" --cache-dir "$archive_cache" \
  --format higv2 --manifest-format legacy --password "$PASSWORD" \
  --daemon off --project off --json >/dev/null
"$OLD_ARCHIVE_BIN" pack "$ARCHIVE_ROOT/source" \
  --output "$ARCHIVE_ROOT/higv2-compact-password.hig" --cache-dir "$archive_cache" \
  --format higv2 --manifest-format compact --password "$PASSWORD" \
  --daemon off --project off --json >/dev/null
"$OLD_ARCHIVE_BIN" pack "$ARCHIVE_ROOT/source" \
  --output "$ARCHIVE_ROOT/higv2-compact-none.hig" --cache-dir "$archive_cache" \
  --format higv2 --manifest-format compact --encryption none \
  --daemon off --project off --json >/dev/null

printf 'pub fn baseline() -> u8 { 1 }\n' > "$REPOSITORY_ROOT/workspace/src/lib.rs"
printf 'synthetic repository fixture\n' > "$REPOSITORY_ROOT/workspace/README.md"
"$OLD_REPOSITORY_BIN" repo init "$REPOSITORY_ROOT/workspace" --json >/dev/null
"$OLD_REPOSITORY_BIN" repo snapshot "$REPOSITORY_ROOT/workspace" \
  --message baseline --author fixture-generator --json >/dev/null
printf 'pub fn baseline() -> u8 { 2 }\n' > "$REPOSITORY_ROOT/workspace/src/lib.rs"
"$OLD_REPOSITORY_BIN" repo snapshot "$REPOSITORY_ROOT/workspace" \
  --message one-byte-change --author fixture-generator --json >/dev/null

if [[ -e "$REPOSITORY_ROOT/workspace/.hig/repository/HEAD" || \
      -e "$REPOSITORY_ROOT/workspace/.hig/repository/refs/heads/main" ]]; then
  echo "repository fixture was not produced by a legacy direct-HEAD writer" >&2
  exit 1
fi
test -f "$REPOSITORY_ROOT/workspace/.hig/repository/refs/HEAD"

(
  cd "$ROOT"
  find fixtures/archives/v1.9.6 -type f ! -name SHA256SUMS -print0 | \
    LC_ALL=C sort -z | xargs -0 bash -c 'for path; do if command -v sha256sum >/dev/null 2>&1; then sha256sum "$path"; else shasum -a 256 "$path"; fi; done' _ \
    > "$ARCHIVE_ROOT/SHA256SUMS"
  find fixtures/repositories/v1.10.0-direct-head -type f ! -name SHA256SUMS -print0 | \
    LC_ALL=C sort -z | xargs -0 bash -c 'for path; do if command -v sha256sum >/dev/null 2>&1; then sha256sum "$path"; else shasum -a 256 "$path"; fi; done' _ \
    > "$REPOSITORY_ROOT/SHA256SUMS"
)

printf '%s\n' 'v1.9.6 archive and v1.10.0 direct-HEAD repository fixtures generated'
