#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
HIG_BIN=${HIG_BIN:-"$ROOT/target/debug/hig"}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/hig-compat.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

if [[ ! -x "$HIG_BIN" ]]; then
  cargo build -q -p hig-cli --manifest-path "$ROOT/Cargo.toml"
fi

INPUT="$WORK/input"
CACHE="$WORK/cache"
mkdir -p "$INPUT/src/nested" "$CACHE"
printf 'alpha\n' > "$INPUT/a.txt"
printf 'fn beta() -> u8 { 2 }\n' > "$INPUT/src/nested/b.rs"
: > "$INPUT/empty.bin"

PASSWORD='hig-compatibility-only'
run_pack() {
  local output=$1
  shift
  "$HIG_BIN" pack "$INPUT" --output "$output" --cache-dir "$CACHE" \
    --daemon off --project off --speed balanced --json "$@" >/dev/null
}

run_unpack_and_compare() {
  local archive=$1
  local output=$2
  shift 2
  "$HIG_BIN" unpack "$archive" --output-dir "$output" "$@" >/dev/null
  diff -ru "$INPUT" "$output"
}

run_pack "$WORK/v1.hig" --format higv1 --password "$PASSWORD"
run_pack "$WORK/v2-legacy.hig" --format higv2 --manifest-format legacy --password "$PASSWORD"
run_pack "$WORK/v2-compact.hig" --format higv2 --manifest-format compact --password "$PASSWORD"
run_pack "$WORK/v2-none.hig" --format higv2 --manifest-format compact --encryption none

run_unpack_and_compare "$WORK/v1.hig" "$WORK/out-v1" --password "$PASSWORD"
run_unpack_and_compare "$WORK/v2-legacy.hig" "$WORK/out-v2-legacy" --password "$PASSWORD"
run_unpack_and_compare "$WORK/v2-compact.hig" "$WORK/out-v2-compact" --password "$PASSWORD"
run_unpack_and_compare "$WORK/v2-none.hig" "$WORK/out-v2-none"

cp "$WORK/v2-compact.hig" "$WORK/corrupted-manifest.hig"
perl -0777 -i -pe 'substr($_, 64, 1) = chr(ord(substr($_, 64, 1)) ^ 1)' \
  "$WORK/corrupted-manifest.hig"
if "$HIG_BIN" inspect "$WORK/corrupted-manifest.hig" --password "$PASSWORD" \
  >/dev/null 2>&1; then
  echo "corrupted-manifest: FAIL" >&2
  exit 1
fi

if "$HIG_BIN" inspect "$WORK/v2-compact.hig" --password wrong >/dev/null 2>&1; then
  echo "wrong-password: FAIL" >&2
  exit 1
fi
cp "$WORK/v2-compact.hig" "$WORK/truncated.hig"
truncate -s -1 "$WORK/truncated.hig"
if "$HIG_BIN" unpack "$WORK/truncated.hig" --output-dir "$WORK/out-truncated" \
  --password "$PASSWORD" >/dev/null 2>&1; then
  echo "truncated-archive: FAIL" >&2
  exit 1
fi
cp "$WORK/v2-compact.hig" "$WORK/unsupported-version.hig"
printf '\003\000\000\000' | dd of="$WORK/unsupported-version.hig" bs=1 seek=8 conv=notrunc >/dev/null 2>&1
if "$HIG_BIN" inspect "$WORK/unsupported-version.hig" --password "$PASSWORD" >/dev/null 2>&1; then
  echo "unsupported-version: FAIL" >&2
  exit 1
fi

printf 'existing-target' > "$WORK/existing-target.hig"
if "$HIG_BIN" migrate "$WORK/truncated.hig" --output "$WORK/existing-target.hig" \
  --password "$PASSWORD" --overwrite --json >/dev/null 2>&1; then
  echo "failed-migration-publication: FAIL" >&2
  exit 1
fi
test "$(cat "$WORK/existing-target.hig")" = "existing-target"

"$HIG_BIN" migrate "$WORK/v1.hig" --output "$WORK/migrated-none.hig" \
  --password "$PASSWORD" --encryption none --json >/dev/null
run_unpack_and_compare "$WORK/migrated-none.hig" "$WORK/out-migrated-none"

"$HIG_BIN" migrate "$WORK/v1.hig" --output "$WORK/migrated-password.hig" \
  --password "$PASSWORD" --target-password 'hig-new-password' \
  --encryption password --json >/dev/null
run_unpack_and_compare "$WORK/migrated-password.hig" "$WORK/out-migrated-password" \
  --password 'hig-new-password'

if [[ -n "${HIG_COMPAT_OLD_BIN:-}" ]]; then
  if [[ ! -x "$HIG_COMPAT_OLD_BIN" ]]; then
    echo "historical-binary: FAIL (not executable): $HIG_COMPAT_OLD_BIN" >&2
    exit 1
  fi
  echo "historical-binary: $($HIG_COMPAT_OLD_BIN --version 2>/dev/null || true)"
  "$HIG_COMPAT_OLD_BIN" pack "$INPUT" --output "$WORK/historical-v1.hig" \
    --format higv1 --password "$PASSWORD" --daemon off --project off --json >/dev/null
  run_unpack_and_compare "$WORK/historical-v1.hig" "$WORK/out-historical" --password "$PASSWORD"
  echo "historical-binary: PASS"
else
  echo "historical-binary: NOT_RUN (set HIG_COMPAT_OLD_BIN to run it)"
fi

echo "hig-compatibility-matrix: PASS"
