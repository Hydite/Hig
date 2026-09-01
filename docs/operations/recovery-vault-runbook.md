# Recovery Vault Operator Runbook

## Recovery Boundary

Recovery Vault restores verified immutable repository bytes that were published
by a completed capture. It does not perform filesystem forensics and does not
reconstruct source bytes from neural, semantic, hash, or cache records. Recovery
is possible after direct deletion, recycle-bin or Trash emptying, workspace
volume loss, and primary Vault loss only while at least one captured Vault copy
and the corresponding external authentication custody remain readable and
cryptographically verifiable. Hashes, indexes, tombstones, or semantic records
without retained object bytes cannot reconstruct a file.

The schema-1 production profile requires external encryption. Place every Vault
root on an encrypted account, volume, or equivalent managed storage domain. A
local mirror on the same physical device does not satisfy media-loss protection.

## Initial Protection

Set explicit paths so the operational authority is unambiguous:

```bash
export HIG_VAULT=/encrypted/local/hig-recovery
export HIG_MIRROR=/encrypted/independent/hig-recovery
export HIG_RECOVERY_AUTH_DIR=/protected/local/hig-recovery-auth

hig repo init /work/project --json
hig repo snapshot /work/project --message "protected baseline" --json
hig recovery init --vault-root "$HIG_VAULT" --mirror "$HIG_MIRROR" --json
hig recovery capture /work/project --revision HEAD --vault-root "$HIG_VAULT" --json
hig recovery status --vault-root "$HIG_VAULT" --json
hig recovery scrub --vault-root "$HIG_VAULT" --json
hig recovery auth export --vault-root "$HIG_VAULT" \
  --output /offline/hig-recovery-custody.json --json
```

Do not treat a point as media-loss protected unless `durability` is
`protected`, `durability_lag_points` is zero, and scrub succeeds for the primary
and every configured mirror.

The custody bundle contains raw recovery authentication key material. Store it
outside every Vault and source volume, encrypt it with an independently managed
control, restrict it to the recovery operator, and never place it in source
control, logs, tickets, or an IDE prompt. `HIG_RECOVERY_AUTH_DIR` contains the
live versioned lineage keys plus monotonic state and audit checkpoints. Back it
up independently; copying only a Vault is insufficient for authenticated
recovery on a new host.

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
export HIG_RECOVERY_AUTH_DIR=/protected/local/hig-recovery-auth

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

If the host authentication directory was also lost, restore it from a protected
backup or import the matching custody bundle before any verification or
promotion:

```bash
hig recovery auth import --vault-root "$HIG_SURVIVOR" \
  --input /offline/hig-recovery-custody.json --json
```

Import verifies Vault identity, local state, the monotonic checkpoint, and the
authenticated audit head. It rejects a bundle for another Vault, a stale
checkpoint, conflicting key material, or modified Vault state.

## Authentication Migration and Rotation

Vaults created before authenticated state publication are read only after an
explicit offline migration. Stop all writers, preserve a storage snapshot, and
run:

```bash
hig recovery migrate-auth --vault-root "$HIG_VAULT" --json
hig recovery audit --vault-root "$HIG_VAULT" --json
hig recovery scrub --vault-root "$HIG_VAULT" --json
hig recovery auth export --vault-root "$HIG_VAULT" \
  --output /offline/hig-recovery-custody.json --json
```

Migration verifies checked control documents, registration identity, every
published repository graph, mirror equivalence, and audit pairing before it
creates authentication state. It is explicit, resumable, and idempotent.

Rotate a live lineage only after every configured mirror is online and scrubbed:

```bash
hig recovery auth rotate --vault-root "$HIG_VAULT" --json
hig recovery scrub --vault-root "$HIG_VAULT" --json
hig recovery auth export --vault-root "$HIG_VAULT" \
  --output /offline/hig-recovery-custody-after-rotation.json --json
```

Rotation updates mirrors before the primary and dual-authenticates each
cross-key transition. Interrupted runs are retryable. Old key files are retained
to preserve offline custody compatibility; their deletion requires a separate
reviewed custody policy and is not performed by rotation.

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
`committed` or `failed` event. Events are covered by an append-only BLAKE3 chain
whose head is authenticated and checkpointed outside the Vault. Deletion,
replacement, reordering, or rollback is rejected even if an attacker recomputes
the ordinary checked-JSON checksum. A prepared event without a terminal record
means the process was interrupted; inspect the authenticated state and verify
affected points before retrying. Never delete audit events to make status appear
clean.

## IDE and MCP Policy

Keep `HIG_MCP_ALLOWED_ROOTS` restricted to explicit workspace, Vault, mirror,
authentication, and restore roots. Leave `HIG_MCP_ALLOW_ANY_PATH` and
`HIG_MCP_ALLOW_GLOBAL_RECOVERY` unset in production. The MCP adapter resolves
physical ancestors, revalidates paths immediately before process creation, and
passes a fail-closed root capability to the HIG child for a second check. It
requires an explicit Vault root, strictly types destructive booleans, defaults
GC to report-only, and defaults restore to no-overwrite. Custody export/import
and legacy authentication migration are deliberately CLI-only so an IDE agent
cannot retrieve raw recovery keys.

## Acceptance Drill

Run the executable form of this procedure with the production binary:

```bash
HIG_BIN=/path/to/hig node scripts/recovery-vault-runbook-test.mjs
```

The drill succeeds only after exact restore from a replacement mirror following
loss of the source, original primary, and promoted survivor.
