# HIG Format Compatibility Matrix

## Scope

This document defines the compatibility gate for the portable `.hig` archive
format and for the in-place archive migration command. It is deliberately
separate from CLI implementation details, cache layout, daemon protocol, and
IDE snapshot state.

The gate has two layers:

1. In-tree Rust tests generate synthetic archives and verify every reader and
   writer combination supported by the current release.
2. `scripts/hig-compatibility-matrix.sh` runs the same matrix through the CLI
   and can additionally consume a historical CLI supplied through
   `HIG_COMPAT_OLD_BIN`.

No private project, benchmark corpus, user path, or secret is used by this
matrix. The password used by the synthetic cases is test-only.

## Required Matrix

| Case | Writer | Encryption | Manifest | Required result |
| --- | --- | --- | --- | --- |
| v1-password | HIGV1 | password | legacy bincode | inspect, unpack, and migrate succeed |
| v2-legacy-password | HIGV2 | password | legacy bincode | inspect and unpack succeed |
| v2-compact-password | HIGV2 | password | compact `HCM1` | inspect and unpack succeed |
| v2-compact-none | HIGV2 | none | compact `HCM1` | inspect and unpack succeed without a password |
| v1-to-v2-none | migration | none target | compact target | exact content and byte counts preserved |
| v1-to-v2-password | migration | new password | compact target | source and target passwords are independent |
| corrupted-manifest | any | applicable | applicable | inspection or extraction fails closed |
| wrong-password | encrypted | password | applicable | authentication fails and no target is published |
| unsafe-path | synthetic invalid metadata | applicable | applicable | extraction rejects traversal or absolute paths |
| unsupported-version | synthetic header | applicable | applicable | reader rejects without best-effort decoding |

## Compatibility Rules

- HIGV2 readers MUST reject unknown magic, unsupported version, unsupported
  header flags, malformed lengths, invalid manifest schemas, and authentication
  failures.
- HIGV2 readers MUST continue to read the legacy HIGV2 manifest profile and
  the compact HIGV2 profile.
- HIGV2 readers MAY read HIGV1 archives. The current CLI does so and exposes
  `hig migrate` to publish a verified HIGV2 replacement.
- Migration MUST stage and fully verify the target before publication.
- Migration MUST preserve relative paths, file sizes, file contents, and file
  counts. A failed migration MUST leave an existing target byte-for-byte
  unchanged.
- Format changes that alter cryptographic interpretation, header layout,
  payload decoding, or path semantics require a new magic/version.

## Historical Binary Gate

To test a historical implementation, provide a binary that can produce a
synthetic archive:

```bash
HIG_COMPAT_OLD_BIN=/absolute/path/to/old/hig \
  bash scripts/hig-compatibility-matrix.sh
```

The script records the historical binary version and verifies that the current
reader can inspect, unpack, and migrate its output. It does not treat a
missing historical binary as a pass; the historical portion is reported as
`NOT_RUN`.

## Release Interpretation

The compatibility gate is a correctness gate, not a performance benchmark.
Performance claims must cite a separately controlled corpus, cache state,
volume qualification, and correctness digest. A green format matrix does not
justify a speedup claim.
