# ADR 0007: HIG Recovery Vault

## Status

Accepted for implementation after HIG v1.10.0.

## Context

HIG repository history already provides byte-exact recovery from immutable,
BLAKE3-addressed objects stored under `.hig/repository`. That placement protects
against accidental edits and interrupted snapshots, but it does not survive
deletion of the complete workspace, loss of the workspace volume, or deletion
of the repository directory itself. Project caches and neural or semantic
indexes are insufficient because they are mutable acceleration state and do not
contain an authoritative copy of every source byte.

Recovery must remain possible after ordinary deletion, recycle-bin emptying,
and direct filesystem removal, provided a completed recovery point exists in a
surviving durability domain. The design must preserve compatibility with every
v1.10 repository and must not weaken atomic publication, object verification,
or reachability garbage collection.

## Decision

HIG adds a global Recovery Vault outside the protected workspace. The default
location is platform-specific user application data and may be overridden by
`HIG_RECOVERY_VAULT`. A vault has a versioned catalog and one isolated repository
directory per stable HIG repository identifier. Workspace `.hig/repository`
remains the local source of truth for normal repository commands and remains
readable without the vault.

A capture operation runs under both repository and vault writer locks. It
resolves the selected commit, traverses its complete typed object graph, copies
missing immutable objects to the vault, verifies each destination object by
kind, length, checksum, and object identifier, synchronizes new objects and
their directories, and only then atomically publishes a protected recovery
reference and catalog generation. An interrupted capture can leave unreachable
immutable objects but cannot publish an incomplete recovery point.

Each registration binds the repository identifier to a generated registration
identifier and records canonical source-path history for operator discovery.
Paths are labels, not identities: deleting and recreating a path cannot silently
take ownership of an earlier vault repository.

The primary vault may have zero or more filesystem mirrors. A mirror receives
the same immutable objects and an independently published reference set. A
recovery point reports its achieved durability. A same-volume primary vault
does not satisfy media-loss protection; T0 policy requires at least one verified
mirror on an independent durability domain before reporting a protected state.

Recovery opens the vault repository directly, resolves only its published
protected references, verifies the reachable graph, restores to a newly created
or explicitly approved destination, verifies every reconstructed file, and
publishes no workspace metadata until restore succeeds.

Deletion detection creates a tombstone event; it never deletes recovery data.
Retention is controlled by protected refs, explicit pins, minimum age, minimum
generation count, and quota policy. Vault garbage collection traverses every
protected ref and mirror-required recovery point while holding the writer lock.
It is report-only by default.

## Recovery Contract

- Recovery point objective: the newest successfully published vault capture.
- Recovery time objective: bounded by object verification plus sequential
  reconstruction; target measurements are defined in the completion matrix.
- Byte integrity: every restored regular file is length- and BLAKE3-verified.
- Namespace integrity: paths are normalized and confined beneath the selected
  destination; links and metadata cannot escape it.
- Durability: "captured" means primary vault publication completed;
  "protected" means every policy-required mirror also completed and verified.
- No false promise: unsnapshotted bytes, deleted vault copies, and storage media
  that share one failure domain cannot be recovered by hashes or indexes.

Filesystem fidelity is part of the immutable repository object contract rather
than a best-effort restore option. Current snapshots use file schema 6 and tree
schema 5. They retain regular-file and symlink type, exact file bytes, directory
structure, modification time, permission mode, hardlink identity, allocated
sparse extents, Unix owner/group identity, managed extended attributes, and
platform access-control metadata. Readers keep explicit decoders for every
earlier file and tree schema;
missing fields in an older object mean that the older snapshot did not capture
that property, not that the reader may synthesize it.

Metadata capture and restore fail closed. Attribute names and ACL payloads are
bounded and validated, metadata-only changes create a new repository version,
and restored metadata is read back before a restore is accepted. Platform-owned
attributes that cannot be safely replayed are excluded explicitly. macOS stores
extended ACL text and user-managed xattrs, including resource forks. Linux
stores raw POSIX access/default ACL xattrs and user-managed xattrs. Windows
stores owner, primary group, DACL, and DACL inheritance protection. Windows SACL
data is excluded because reading and applying audit policy requires privileges
that ordinary IDE processes do not possess. Windows alternate data streams are
not covered by this schema and therefore remain an open production-fidelity
gate; the product must not claim complete NTFS stream recovery until that gate
is implemented and proven natively.

## Alternatives Considered

### Use the cache as recovery storage

Rejected. Cache compaction, eviction, policy changes, and partial materialization
make it unsuitable as authoritative history.

### Move `.hig/repository` out of the workspace

Rejected as the only mechanism. It would break existing repositories and tools,
and a single external location still cannot establish independent durability.

### Replicate the immutable repository into an external vault

Accepted. It preserves v1.10 compatibility, reuses verified object semantics,
supports incremental transfer, and permits independent mirrors without placing
the correctness of ordinary repository operations on remote availability.

### Depend on operating-system undelete or block recovery

Rejected. Secure deletion, TRIM, filesystem reuse, encryption, and remote
filesystems make physical undelete nondeterministic and outside HIG's contract.

## Consequences

- A workspace can be completely absent when recovery begins.
- Capture consumes additional I/O and durable capacity proportional to new
  unique objects; unchanged chunks are not recopied.
- Operators must place a mirror in another failure domain for disk-loss claims.
- Vault formats and catalog schemas become long-term compatibility surfaces and
  require immutable fixtures before release.
- A restore to a different operating-system ACL family must fail when stored
  platform metadata cannot be represented exactly; silent translation is not
  permitted by the first production profile.
- Encryption at rest, key custody, anti-ransomware controls, and off-host
  replication are separate policy layers; local permissions alone are not a
  substitute for them.
