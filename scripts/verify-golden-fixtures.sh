#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
HIG_BIN=${HIG_BIN:-"$ROOT/target/release/hig"}
ARCHIVE_ROOT="$ROOT/fixtures/archives/v1.9.6"
REPOSITORY_ROOT="$ROOT/fixtures/repositories/v1.10.0-direct-head"
PASSWORD='hig-public-fixture-v1'
WORK=$(mktemp -d "${TMPDIR:-/tmp}/hig-golden-verify.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

test -x "$HIG_BIN"
test -f "$ARCHIVE_ROOT/SHA256SUMS"
test -f "$REPOSITORY_ROOT/SHA256SUMS"

verify_checksums() {
  local manifest=$1
  while read -r expected path; do
      path=${path%$'\r'}
      local actual
      if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$ROOT/$path" | awk '{print $1}')
      else
      actual=$(shasum -a 256 "$ROOT/$path" | awk '{print $1}')
      fi
      if [[ "$actual" != "$expected" ]]; then
        echo "$path: checksum mismatch" >&2
        return 1
      fi
      echo "$path: OK"
  done < "$ROOT/$manifest"
}

verify_checksums "fixtures/archives/v1.9.6/SHA256SUMS"
verify_checksums "fixtures/repositories/v1.10.0-direct-head/SHA256SUMS"

verify_archive() {
  local name=$1
  local encrypted=$2
  local source="$ARCHIVE_ROOT/$name"
  local output="$WORK/out-${name%.hig}"
  local migrated="$WORK/migrated-${name}"
  if [[ "$encrypted" == true ]]; then
    "$HIG_BIN" inspect "$source" --password "$PASSWORD" --json >/dev/null
    "$HIG_BIN" unpack "$source" --output-dir "$output" --password "$PASSWORD" >/dev/null
    "$HIG_BIN" migrate "$source" --output "$migrated" --password "$PASSWORD" \
      --encryption none --json >/dev/null
  else
    "$HIG_BIN" inspect "$source" --json >/dev/null
    "$HIG_BIN" unpack "$source" --output-dir "$output" >/dev/null
    "$HIG_BIN" migrate "$source" --output "$migrated" --encryption none --json >/dev/null
  fi

  diff -ru "$ARCHIVE_ROOT/source" "$output"
  "$HIG_BIN" unpack "$migrated" --output-dir "$WORK/migrated-out-${name%.hig}" >/dev/null
  diff -ru "$ARCHIVE_ROOT/source" "$WORK/migrated-out-${name%.hig}"
}

verify_archive higv1-password.hig true
verify_archive higv2-legacy-password.hig true
verify_archive higv2-compact-password.hig true
verify_archive higv2-compact-none.hig false

cp -R "$REPOSITORY_ROOT/workspace" "$WORK/repository"
test -f "$WORK/repository/.hig/repository/refs/HEAD"
test ! -e "$WORK/repository/.hig/repository/HEAD"
hash_objects() {
  local output=$1
  find "$WORK/repository/.hig/repository/objects" -type f -print0 | LC_ALL=C sort -z | \
    xargs -0 bash -c 'for path; do if command -v sha256sum >/dev/null 2>&1; then sha256sum "$path"; else shasum -a 256 "$path"; fi; done' _ \
    | sed "s#$WORK/repository/.hig/repository/objects#objects#" > "$output"
}

hash_objects "$WORK/objects-before"
"$HIG_BIN" repo verify "$WORK/repository" --json >/dev/null
"$HIG_BIN" repo migrate "$WORK/repository" --json > "$WORK/migrate-first.json"
test "$(jq -r '.from_legacy' "$WORK/migrate-first.json")" = true
test "$(cat "$WORK/repository/.hig/repository/HEAD")" = "ref: refs/heads/main"
test -f "$WORK/repository/.hig/repository/refs/heads/main"
hash_objects "$WORK/objects-after"
diff -u "$WORK/objects-before" "$WORK/objects-after"
"$HIG_BIN" repo migrate "$WORK/repository" --json > "$WORK/migrate-second.json"
test "$(jq -r '.from_legacy' "$WORK/migrate-second.json")" = false
"$HIG_BIN" repo verify "$WORK/repository" --json >/dev/null
"$HIG_BIN" repo restore "$WORK/repository" --revision HEAD \
  --output-dir "$WORK/repository-restored" --json >/dev/null
diff -ru --exclude=.hig "$WORK/repository" "$WORK/repository-restored"

echo "hig-golden-fixtures: PASS"
