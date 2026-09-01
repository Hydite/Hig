# Recovery Vault Operator Runbook

## Recovery Boundary

Recovery Vault restores verified immutable repository bytes that were published
by a completed capture. It does not perform filesystem forensics and does not
reconstruct source bytes from neural, semantic, hash, or cache records. Recovery
is possible after direct deletion, recycle-bin or Trash emptying, workspace
volume loss, and primary Vault loss only while at least one captured Vault copy
remains readable and cryptographically verifiable.

The schema-1 production profile requires external encryption. Place every Vault
root on an encrypted account, volume, or equivalent managed storage domain. A
local mirror on the same physical device does not satisfy media-loss protection.

## Initial Protection

Set explicit paths so the operational authority is unambiguous:

```bash
export HIG_VAULT=/encrypted/local/hig-recovery
export HIG_MIRROR=/encrypted/independent/hig-recovery

hig repo init /work/project --json
hig repo snapshot /work/project --message "protected baseline" --json
hig recovery init --vault-root "$HIG_VAULT" --mirror "$HIG_MIRROR" --json
hig recovery capture /work/project --revision HEAD --vault-root "$HIG_VAULT" --json
hig recovery status --vault-root "$HIG_VAULT" --json
hig recovery scrub --vault-root "$HIG_VAULT" --json
```

Do not treat a point as media-loss protected unless `durability` is
`protected`, `durability_lag_points` is zero, and scrub succeeds for the primary
and every configured mirror.

For an IDE watcher, configure the same Vault explicitly. Managed MCP watcher
status reports `recovery_last_success_at`, `recovery_rpo_lag_ms`,
`recovery_durability`, and `recovery_durability_lag`. An active watcher is not
proof of protection; the last successful capture and durability fields are.

## Workspace Deleted

Stop writers that may recreate the old path. The original workspace is not
needed for discovery, verification, or restore:

```bash
hig recovery status --vault-root "$HIG_VAULT" --json
hig recovery list --vault-root "$HIG_VAULT" --json
hig recovery audit --vault-root "$HIG_VAULT" --json
hig recovery verify <repository-id> <recovery-point-id> \
  --vault-root "$HIG_VAULT" --json
hig recovery restore <repository-id> <recovery-point-id> \
  --vault-root "$HIG_VAULT" \
  --output-dir /work/recovered-project --json
```

Restore refuses an existing destination unless `--overwrite` is explicit.
Prefer a new destination, verify the result, and only then replace operational
paths. Use `--path <repository-relative-path>` for a selected file or subtree;
absolute paths and traversal components are rejected.

## Primary Vault Lost

1. Stop capture, GC, repair, and retention writers.
2. Select one surviving mirror and keep a read-only storage snapshot if the
   surrounding platform supports it.
3. Run `status`, `audit`, and `scrub` directly against the survivor.
4. Restore a representative checkpoint before changing authority.
5. Promote the survivor and replicate it to at least one new independent
   durability domain.

```bash
export HIG_SURVIVOR=/encrypted/independent/hig-recovery
export HIG_REPLACEMENT=/encrypted/new-domain/hig-recovery

hig recovery status --vault-root "$HIG_SURVIVOR" --json
hig recovery audit --vault-root "$HIG_SURVIVOR" --json
hig recovery scrub --vault-root "$HIG_SURVIVOR" --json

hig recovery promote \
  --vault-root "$HIG_SURVIVOR" \
  --mirror "$HIG_REPLACEMENT" \
  --json

hig recovery scrub --vault-root "$HIG_SURVIVOR" --json
export HIG_RECOVERY_VAULT="$HIG_SURVIVOR"
```

Promotion verifies the survivor first, incrementally copies every published
recovery point, and publishes protected status only after every requested new
mirror succeeds. It is retry-safe. Failure before the candidate catalog update
leaves its durability conservative; a completed replacement may contain
unreferenced or captured data that the next retry reuses.

Do not promote a Vault with scrub errors or recovery points pending deletion.
Do not point a replacement mirror at an unrelated nonempty Vault; conflicting
registration identity, path history, recovery points, or tombstones fail closed.

## Corruption and Repair

Never repair from an unverified or unconfigured location:

```bash
hig recovery scrub --vault-root "$HIG_VAULT" --json
hig recovery repair <repository-id> <recovery-point-id> \
  --vault-root "$HIG_VAULT" --mirror "$HIG_MIRROR" --json
hig recovery verify <repository-id> <recovery-point-id> \
  --vault-root "$HIG_VAULT" --json
```

Repair accepts only a configured mirror whose matching point verifies. Object
identity or commit disagreement is corruption and stops the operation.

## Retention and Capacity

Inspect before changing policy. GC is report-only unless `--apply` is explicit:

```bash
hig recovery policy show --vault-root "$HIG_VAULT" --json
hig recovery gc --vault-root "$HIG_VAULT" --json
hig recovery gc --vault-root "$HIG_VAULT" --apply --json
```

Pins and minimum count/age constraints remain authoritative under quota. A
quota that cannot be met without violating them reports `policy_satisfied=false`
instead of deleting protected data. After capacity exhaustion, free space,
rerun the interrupted operation, then run audit and scrub.

## Audit Interpretation

Every mutation and restore has one durable `prepared` event and at most one
`committed` or `failed` event. A prepared event without a terminal record means
the process was interrupted; it is not proof that publication failed or
succeeded. Inspect current catalog generation and verify affected points before
retrying. Never delete audit events to make status appear clean.

## IDE and MCP Policy

Keep `HIG_MCP_ALLOWED_ROOTS` restricted to explicit workspace, Vault, mirror,
and restore roots. Leave `HIG_MCP_ALLOW_ANY_PATH` and
`HIG_MCP_ALLOW_GLOBAL_RECOVERY` unset in production. The MCP adapter resolves
physical ancestors to reject symlink escapes, requires an explicit Vault root,
strictly types destructive booleans, defaults GC to report-only, and defaults
restore to no-overwrite.

## Acceptance Drill

Run the executable form of this procedure with the production binary:

```bash
HIG_BIN=/path/to/hig node scripts/recovery-vault-runbook-test.mjs
```

The drill succeeds only after exact restore from a replacement mirror following
loss of the source, original primary, and promoted survivor.
