# HIG Recovery Vault Design

## Scope

Recovery Vault extends HIG repository history so a completed recovery point can
restore data after the source file, recycle bin, workspace, or source volume is
gone. It does not attempt forensic undelete and does not infer bytes from a
neural index. The byte source is the verified immutable object graph.

## Functional Requirements

1. Register an existing HIG repository in a global vault using stable identity.
2. Capture a selected commit and all reachable objects incrementally.
3. Publish a recovery point only after complete destination verification.
4. List repositories and recovery points without the source workspace.
5. Restore a whole tree or selected path from a vault by repository ID.
6. Verify, scrub, and repair a primary vault from a verified mirror.
7. Record source deletion/path reuse as events without deleting retained data.
8. Pin/unpin recovery points and enforce versioned retention and quota policy.
9. Mirror captures to independent filesystem roots with explicit durability
   status and retry-safe reconciliation.
10. Expose equivalent CLI, MCP, and IDE-safe operations with path confinement.

## Non-Functional Requirements

| Property | T0 target |
|---|---|
| Correctness | Exact bytes for every file in a published recovery point |
| Atomicity | No incomplete recovery point becomes discoverable |
| RPO | Last successful capture; watcher lag is measured and reported |
| RTO | 1 GiB sequential restore under 5 minutes on qualified local SSD |
| Integrity | 100% reachable-object and restored-file verification |
| Availability | Restore works with source absent and one configured replica down |
| Durability | Protected status requires two independently verified copies |
| Compatibility | v1.10 repository remains readable and migrates without rewrite |
| Capacity | Quota cannot evict pinned or minimum-retention recovery points |
| Concurrency | One writer per repository/vault; readers see a stable generation |
| Auditability | Every mutation has timestamp, generation, actor, and outcome |

"T0" is an HIG engineering tier, not a claim of military, government, safety,
or cryptographic certification.

## On-Disk Layout

```text
recovery-vault/
  config.json
  catalog.json
  locks/write.lock
  repositories/<repository-id>/
    registration.json
    repository/
      config.json
      HEAD
      objects/aa/bb...
      refs/recovery/<recovery-point-id>
      locks/write.lock
  events/<operation-id>.<prepared|committed|failed>.json
```

Catalog and registration files use explicit schema numbers. Object bytes retain
the existing repository object format. Mutable JSON documents are written to a
same-directory temporary file, flushed, atomically renamed, and followed by a
directory sync. Recovery refs are the final publication record. Audit documents
are immutable checked JSON files published without replacement. Each operation
has exactly one durable `prepared` event and at most one terminal event; a
missing terminal event is an explicit interruption record.

## Capture State Machine

```text
requested -> locked -> traversed -> copied -> object-verified
          -> primary-published -> mirrors-published -> protected
```

Failure before `primary-published` leaves no visible recovery point. Failure
after primary publication yields `captured`, not `protected`, and reconciliation
continues idempotently. The same object ID may be copied repeatedly; conflicting
bytes are corruption and stop publication.

## Failure and Attack Matrix

| Event | Required behavior |
|---|---|
| File or workspace deleted | Restore by repository ID with source absent |
| Source volume lost | Restore from primary vault or independent mirror |
| Primary vault lost | Promote verified mirror without source dependency |
| Mirror unavailable | Primary capture may finish but cannot report protected |
| Capture interrupted | No ref to an incomplete graph; later retry is idempotent |
| Restore interrupted | Destination is not reported complete; retry is safe |
| GC interrupted | Published refs remain; unreachable objects may remain |
| Corrupt primary object | Verification fails closed; repair only from matching mirror |
| Corrupt mirror object | Mirror loses verified status; primary is not overwritten |
| Path reused by new project | New registration cannot claim previous repository ID |
| Concurrent capture/GC | Locks serialize mutable publication and deletion |
| Ransomware with user rights | Detection/audit helps; local writable copies are not sufficient |
| Malicious object/path | Typed decoding, size limits, checksum, and path confinement fail closed |

## Filesystem Fidelity Contract

The repository wire format is append-only by schema. File schema 7 and tree
schema 6 are the current writers; file schemas 1 through 6 and tree schemas 1
through 5 remain explicit read paths. A newer reader never rewrites an old
reachable object solely to upgrade its schema.

| Property | macOS | Linux/Android | Windows |
|---|---|---|---|
| Exact regular-file bytes | Required and verified | Required and verified | Required and verified |
| Directory and symlink identity | Required | Required | File/directory symlink type retained; native reparse tests required |
| Mode/read-only and mtime | POSIX mode + mtime | POSIX mode + mtime | read-only + mtime |
| Hardlinks | Stable file identity + restore verification | Stable file identity + restore verification | volume/file identity + restore verification |
| Sparse allocation | `SEEK_DATA`/`SEEK_HOLE` when supported | `SEEK_DATA`/`SEEK_HOLE` when supported | allocated-range query + sparse restore |
| Extended attributes and named streams | User-managed xattrs; resource forks included | User-managed xattrs | Named `$DATA` streams on files and directories use ordinary chunk objects |
| ACL | Extended ACL text | Raw POSIX access/default ACL xattrs | Owner, group, DACL, inheritance protection |
| Owner/group | Numeric UID/GID, exact or fail | Numeric UID/GID, exact or fail | Represented by security descriptor |
| Audit ACL | Not a separate namespace | Not a separate namespace | SACL explicitly excluded from ordinary-user profile |

Capture sorts and bounds variable metadata before canonical serialization.
Restore applies metadata only inside the staged destination, reads it back, and
rejects any mismatch before atomic publication. System-managed macOS attributes
(`com.apple.provenance`, `com.apple.macl`, and
`com.apple.system.Security`) are not replayed. Linux ACL xattrs are owned by the
ACL codec and cannot also appear as generic xattrs. Cross-family ACL conversion
is prohibited because a lossy translation would violate exact recovery.

Windows stream names are retained as canonical UTF-16 and confined to one base
object: default streams, path separators, nested stream syntax, and non-`$DATA`
stream types are rejected. Stream bytes are chunked, deduplicated, included in
reachability/GC, and length- and BLAKE3-verified by repository verification and
restore. Reparse-point streams are excluded rather than followed because target
stream capture would violate object identity. Native multi-platform fault/soak
evidence must still close before the completion matrix can be marked complete.

## Security Model

The trust boundary includes the HIG process and vault configuration. Source
files and repository objects are untrusted inputs. Local vault permissions are
owner-only. Mirrors must authenticate storage identity.

The first production profile records
`at_rest_policy=external_encryption_required`. HIG does not encrypt individual
vault objects in this profile. Operators MUST use encrypted operating-system
accounts, encrypted volumes, or an equivalently protected external durability
domain for every primary and mirror. This is a deployment prerequisite, not a
claim that HIG has detected or certified the surrounding storage. HIG provides
authenticated object identity and complete verification, but an unencrypted
vault is not confidential.

HIG MUST NOT place a decryption key in the same vault, manufacture an
unrecoverable local-only key for an unattended IDE watcher, or silently fall
back to plaintext after a requested native-encryption profile fails. A future
native profile requires AEAD protection per immutable object, unique nonce
construction, authenticated mutable metadata, key rotation, schema migration,
external recoverable key custody, and source-and-primary-loss recovery tests.
It requires a distinct schema/profile decision and cannot be introduced as an
unreviewed wrapper around filenames, objects, or refs.

On Unix, HIG verifies that private Vault paths are owned by the effective user,
sets directories to `0700` and control/audit files to `0600`, and refuses
control-file symlinks. On Windows, HIG installs and reads back a protected DACL
for the object owner and `SYSTEM`. Existing Vaults are repaired to the same
policy when opened. These permissions are containment, not a replacement for
the required external encryption or an anti-ransomware boundary.

Every accepted mutation and restore emits a two-record audit transaction. The
prepared record is synchronized before work begins; committed and failed
records are separate immutable files. A hard termination leaves the prepared
record visible as incomplete. The `recovery audit` CLI and
`hig_recovery_audit` MCP tool validate pairing, checksums, identifiers, actor,
catalog generations, and bounded details. Scrub treats malformed audit data as
corruption and reports valid incomplete operations separately.

MCP defaults to configured workspace and vault roots. Destructive operations
are report-only unless `apply` is explicit. Restore refuses overwrite by
default. Secrets, raw object contents, and source file contents are excluded
from routine logs.

## Validation Strategy

- deterministic unit tests for layout, identity, atomic publication, traversal,
  path confinement, retention, quota, and schema rejection;
- fault injection at every state transition, including process termination;
- corruption of header, compressed body, checksum, object ID, ref, and catalog;
- concurrent capture/restore/GC and source mutation races;
- capacity exhaustion and read-only/offline mirror scenarios;
- source-file, workspace, source-volume, and primary-vault deletion drills;
- macOS, Linux, and Windows native package and MCP integration;
- immutable fixtures for every vault/catalog schema and prior HIG repository;
- multi-hour soak with repeated captures, deletions, restores, GC, restart, and
  digest comparison;
- qualified performance comparison against v1.10 repository snapshot/restore.
