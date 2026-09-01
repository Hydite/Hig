#!/usr/bin/env bash
set -Eeuo pipefail

trap 'status=$?; echo "hig-golden-fixtures: FAIL at line $LINENO (exit $status)" >&2' ERR

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
HIG_BIN=${HIG_BIN:-"$ROOT/target/release/hig"}
ARCHIVE_ROOT="$ROOT/fixtures/archives/v1.9.6"
REPOSITORY_ROOT="$ROOT/fixtures/repositories/v1.10.0-direct-head"
RECOVERY_ROOT="$ROOT/fixtures/recovery-vault/v1.10.0-schema1"
PASSWORD='hig-public-fixture-v1'
WORK=$(mktemp -d "${TMPDIR:-/tmp}/hig-golden-verify.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
export HIG_RECOVERY_AUTH_DIR="$WORK/recovery-auth"

test -x "$HIG_BIN"
test -f "$ARCHIVE_ROOT/SHA256SUMS"
test -f "$REPOSITORY_ROOT/SHA256SUMS"
test -f "$RECOVERY_ROOT/SHA256SUMS"

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
verify_checksums "fixtures/recovery-vault/v1.10.0-schema1/SHA256SUMS"

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

cp -R "$RECOVERY_ROOT/vault" "$WORK/recovery-vault"
RECOVERY_REPOSITORY_ID=$(jq -r '.repository_id' "$RECOVERY_ROOT/fixture.json")
RECOVERY_POINT_ID=$(jq -r '.recovery_point_id' "$RECOVERY_ROOT/fixture.json")
RECOVERY_REPOSITORY="$WORK/recovery-vault/repositories/$RECOVERY_REPOSITORY_ID/.hig/repository"
find "$RECOVERY_REPOSITORY/objects" -type f -print0 | LC_ALL=C sort -z | \
  xargs -0 bash -c 'for path; do if command -v sha256sum >/dev/null 2>&1; then sha256sum "$path"; else shasum -a 256 "$path"; fi; done' _ \
  | sed "s#$RECOVERY_REPOSITORY/objects#objects#" > "$WORK/recovery-objects-before"

test "$(jq -r '.schema' "$WORK/recovery-vault/config.json")" = 1
test "$(jq -r '.payload.schema' "$WORK/recovery-vault/config.json")" = 1
test "$(jq -r '.schema' "$WORK/recovery-vault/catalog.json")" = 1
test "$(jq -r '.payload.schema' "$WORK/recovery-vault/catalog.json")" = 1
test "$(jq -r '.payload.retention.schema' "$WORK/recovery-vault/config.json")" = 1
test "$(jq -r '.payload.at_rest_policy' "$WORK/recovery-vault/config.json")" = external_encryption_required
test "$(find "$WORK/recovery-vault/events" -name '*.json' -print0 | xargs -0 -n1 jq -r '.payload.schema' | sort -u)" = 1

if "$HIG_BIN" recovery list --vault-root "$WORK/recovery-vault" --json \
  >"$WORK/recovery-unsealed-list.json" 2>"$WORK/recovery-unsealed-list.err"; then
  echo "legacy Recovery Vault was accepted without explicit authentication migration" >&2
  exit 1
fi
grep -q 'migrate-auth' "$WORK/recovery-unsealed-list.err"

"$HIG_BIN" recovery migrate-auth --vault-root "$WORK/recovery-vault" --json \
  > "$WORK/recovery-migrate-first.json"
test "$(jq -r '.created' "$WORK/recovery-migrate-first.json")" = true
test "$(jq -r '.verified_repositories' "$WORK/recovery-migrate-first.json")" = 1
test "$(jq -r '.verified_recovery_points' "$WORK/recovery-migrate-first.json")" = 1
test "$(jq -r '.verified_audit_events' "$WORK/recovery-migrate-first.json")" = 4
"$HIG_BIN" recovery migrate-auth --vault-root "$WORK/recovery-vault" --json \
  > "$WORK/recovery-migrate-second.json"
test "$(jq -r '.created' "$WORK/recovery-migrate-second.json")" = false

"$HIG_BIN" recovery auth export --vault-root "$WORK/recovery-vault" \
  --output "$WORK/recovery-custody.json" --json > "$WORK/recovery-custody-export.json"
mv "$HIG_RECOVERY_AUTH_DIR" "$WORK/recovery-auth-lost"
mkdir "$HIG_RECOVERY_AUTH_DIR"
"$HIG_BIN" recovery auth import --vault-root "$WORK/recovery-vault" \
  --input "$WORK/recovery-custody.json" --json > "$WORK/recovery-custody-import.json"
test "$(jq -r '.key_id' "$WORK/recovery-custody-export.json")" = \
  "$(jq -r '.key_id' "$WORK/recovery-custody-import.json")"
test "$(jq -r '.checkpoint_sequence' "$WORK/recovery-custody-export.json")" = \
  "$(jq -r '.checkpoint_sequence' "$WORK/recovery-custody-import.json")"

"$HIG_BIN" recovery list --vault-root "$WORK/recovery-vault" --json > "$WORK/recovery-list.json"
test "$(jq -r '.generation' "$WORK/recovery-list.json")" = 1
test "$(jq -r '.repositories | length' "$WORK/recovery-list.json")" = 1
"$HIG_BIN" recovery policy show --vault-root "$WORK/recovery-vault" --json >/dev/null
"$HIG_BIN" recovery audit --vault-root "$WORK/recovery-vault" --json > "$WORK/recovery-audit.json"
test "$(jq -r '.incomplete_operation_ids | length' "$WORK/recovery-audit.json")" = 0
"$HIG_BIN" recovery verify "$RECOVERY_REPOSITORY_ID" "$RECOVERY_POINT_ID" \
  --vault-root "$WORK/recovery-vault" --json >/dev/null
"$HIG_BIN" recovery scrub --vault-root "$WORK/recovery-vault" --json > "$WORK/recovery-scrub.json"
test "$(jq -r '.healthy' "$WORK/recovery-scrub.json")" = true
"$HIG_BIN" recovery restore "$RECOVERY_REPOSITORY_ID" "$RECOVERY_POINT_ID" \
  --vault-root "$WORK/recovery-vault" \
  --output-dir "$WORK/recovery-restored" --json >/dev/null
diff -ru "$RECOVERY_ROOT/expected" "$WORK/recovery-restored"

find "$RECOVERY_REPOSITORY/objects" -type f -print0 | LC_ALL=C sort -z | \
  xargs -0 bash -c 'for path; do if command -v sha256sum >/dev/null 2>&1; then sha256sum "$path"; else shasum -a 256 "$path"; fi; done' _ \
  | sed "s#$RECOVERY_REPOSITORY/objects#objects#" > "$WORK/recovery-objects-after"
diff -u "$WORK/recovery-objects-before" "$WORK/recovery-objects-after"

echo "hig-golden-fixtures: PASS"
