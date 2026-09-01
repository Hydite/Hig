# Recovery Vault Schema and Migration Contract

## Status

This document defines the compatibility contract for the first production
Recovery Vault format shipped with HIG v1.10.0. The vault schema is independent
from the archive format and from immutable repository object schemas.

## Versioned Documents

The following persisted structures carry explicit schema identifiers:

| Structure | Current schema | Compatibility authority |
|---|---:|---|
| Checked JSON envelope | 1 | `CheckedDocument.schema` |
| Vault configuration | 1 | `RecoveryVaultConfig.schema` |
| Retention policy | 1 | `RecoveryRetentionPolicy.schema` |
| Catalog | 1 | `RecoveryCatalog.schema` |
| Registration, point, tombstone | 1 | Each record's `schema` field |
| Audit event | 1 | `RecoveryAuditEvent.schema` |
| Vault identity and state seal | 1 | Authenticated state contract |
| Pending state transition | 1 | Dual-MAC transition contract |
| Audit-chain entry/head/pending | 1 | Authenticated audit contract |
| Authentication custody bundle | 1 | Custody bundle contract |
| Repository objects | Per object kind | Repository reader contract |

Every mutable JSON file and immutable audit event is wrapped by a schema-1
checked envelope. `payload_blake3` is the lowercase BLAKE3 digest of the compact
JSON serialization of `payload`. Readers verify the envelope schema and digest
before interpreting any payload field.

Checked JSON detects accidental corruption but is not an authenticity boundary.
Authenticated Vaults additionally bind `identity.json`, `config.json`, and
`catalog.json` to a monotonic keyed state seal stored both in the Vault and in
`HIG_RECOVERY_AUTH_DIR`. Mutations use an authenticated pending transition and
copy-on-write publication. Audit events are bound in sequence by BLAKE3 chain
entries; the chain head and pending publication are keyed and externally
checkpointed. Key rotation transitions are authenticated by both the previous
and target lineage keys.

## Schema-1 Reader Rules

1. A reader MUST reject an unknown envelope, configuration, retention, catalog,
   registration, recovery-point, tombstone, audit, identity, state, transition,
   audit-chain, or custody schema.
2. A reader MUST reject a checksum mismatch before using the document.
3. Missing fields are accepted only where the schema explicitly defines a
   default. Schema 1 defaults absent `retention` to the non-expiring protected
   policy, absent `at_rest_policy` to `external_encryption_required`, absent
   recovery-point `state` to `available`, and absent registration `tombstones`
   to an empty list.
4. Unknown fields may be retained by external tooling but do not grant new HIG
   semantics. Security or deletion behavior cannot be activated by an unknown
   field.
5. Opening a schema-1 vault MUST NOT rewrite immutable repository objects or
   silently advance a document schema.
6. List, verify, audit, policy inspection, and scrub MUST work without the
   original workspace. Restore may append its normal audit transaction but MUST
   not rewrite pre-existing immutable objects.
7. An authenticated reader MUST fail closed when its lineage key, external
   checkpoint, state seal, or audit head is missing, stale, conflicting, or has
   an invalid MAC. It MUST NOT recreate authentication from Vault-local data.
8. A mirror MAY verify and restore but MUST NOT perform primary mutations until
   a verified promotion changes its authenticated role and primary binding.

## Migration Rules

The original schema-1 fixture predates authenticated state and audit
publication. Current readers intentionally reject it until the operator runs
`hig recovery migrate-auth`. Migration verifies every checked control document,
registration, published recovery ref, reachable repository object, mirror
policy/catalog relationship, and audit pair before creating identities, keyed
state seals, external checkpoints, and audit chains. Migration is resumable and
idempotent and never rewrites immutable repository objects.

A future schema change MUST satisfy all of the following before a writer is
released:

1. Add an immutable fixture for every prior readable vault schema; replacing or
   regenerating an older fixture is prohibited.
2. Specify whether the change is reader-only, append-compatible, or requires an
   explicit migration command. A required migration MUST NOT occur as a side
   effect of list, verify, scrub, or restore.
3. Write changed control documents copy-on-write, sync them, verify their
   checksums, and atomically publish one catalog generation. Failure before
   publication leaves the old schema authoritative.
4. Never rewrite immutable repository objects unless their own versioned object
   contract requires a separately reviewed migration.
5. Preserve protected refs, pins, tombstones, retention meaning, registration
   identity, source-path history, and audit history exactly.
6. Prove interruption recovery before and after every publication boundary and
   retain an auditable migration operation.
7. Verify source-absent and primary-loss restore from the migrated vault and an
   independently migrated mirror.
8. Document downgrade behavior. If an older writer cannot safely operate on the
   new schema, it MUST fail closed rather than partially update the vault.

## Immutable Fixture Gate

`fixtures/recovery-vault/v1.10.0-schema1` is the canonical schema-1 fixture. It
contains checked configuration, catalog, audit events, one protected repository
ref, immutable repository objects inherited from the cross-platform v1.10.0
repository fixture, expected restored files, and a SHA-256 manifest.

`scripts/verify-golden-fixtures.sh` verifies the immutable manifest, proves the
legacy copy is rejected before explicit authentication migration, performs and
repeats migration, exports custody, simulates loss of the live authentication
directory, imports custody, and then verifies policy, list, authenticated audit
history, recovery points, scrub, source-absent restore, expected files, and byte
identity of every pre-existing object before and after the run. This gate runs
in native package CI. Future schema fixtures are additive and MUST be covered by
the same operations.
