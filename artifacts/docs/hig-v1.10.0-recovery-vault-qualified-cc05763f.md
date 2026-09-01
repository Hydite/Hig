# HIG v1.10.0 Recovery Vault Qualified Performance Evidence

Date: 2026-09-01

GitHub Actions run: `33498089911`

Run URL: <https://github.com/Hydite/Hig/actions/runs/33498089911>

Source commit: `cc05763fe3e13734327acb4cfc630776cde6884b`

## Scope and Acceptance Rule

This document records the release-qualified 1 GiB Recovery Vault comparison
on a native macOS arm64 runner. The harness created a deterministic 256-file,
1,073,741,824-byte workspace, captured it into a primary Vault and filesystem
mirror, changed one byte, captured the incremental state, and verified exact
restores from the repository, primary Vault, and surviving mirror after both
the source workspace and primary Vault were removed.

Acceptance required an exact final digest from all restore paths, at least
99% object reuse for the one-byte mutation, bounded incremental writes,
primary and mirror restore RTO below five minutes, peak CLI RSS below 1 GiB,
and combined primary-plus-mirror storage between 1.9 and 2.2 times the logical
workspace size. The workflow validator and an independent downloaded-artifact
validation passed every requirement.

## Qualified Results

| Metric | Result |
|---|---:|
| Fixture | 256 files / 1,073,741,824 bytes |
| Mutation | 1 byte |
| Initial repository snapshot | 21.027 s |
| Initial Vault capture | 73.740 s |
| Incremental repository snapshot | 8.936 s |
| Incremental Vault capture | 45.363 s |
| Direct repository restore | 4.885 s |
| Primary Vault restore | 13.248 s / 77.292 MiB/s |
| Mirror restore after source and primary loss | 6.313 s / 162.207 MiB/s |
| Incremental object reuse | 99.9497% |
| Incremental bytes-written ratio | 0.0134% |
| Peak CLI RSS | 37,732,352 bytes |

The primary Vault occupied 1,075,424,191 bytes and the mirror occupied
1,075,423,751 bytes. Their combined capacity ratio was `2.003133`, reflecting
two independently recoverable copies without multiplying storage for the
one-byte incremental capture.

## Loss and Integrity Results

All exact-restore assertions passed:

- direct repository restore reproduced the final workspace;
- primary Vault restore reproduced the final workspace;
- mirror restore reproduced the final workspace after the source workspace
  and primary Vault were deleted;
- primary verification, full scrub, final GC, and survivor scrub completed;
- the final workspace digest was
  `2dff57a4d6700d18ff5138fca1a6c8a5564599718edb051aadbd05e418f1c98b`.

The primary restore took `2.712100` times the direct repository restore time,
and the mirror-loss restore took `1.292322` times the direct restore time.
Both remained well inside the five-minute 1 GiB RTO contract.

## Machine-Readable Provenance

The retained workflow artifact is named
`recovery-vault-qualified-macos-aarch64-cc05763fe3e13734327acb4cfc630776cde6884b`
and contains `recovery-vault-qualified-macos-aarch64.json`. The report declares
schema `1`, mode `qualified`, status `passed`, platform `darwin`, architecture
`arm64`, and the exact source commit above. Its SHA-256 is
`c027a04c1c1f7380336aa8dd8701c69296e39d710449c509f647e000af780a6c`.

The report was independently validated with
`scripts/validate-recovery-vault-qualified-report.mjs` using the exact expected
commit, `qualified` mode, and a minimum fixture size of 1,073,741,824 bytes.
This evidence supports the Recovery Vault throughput, memory, deduplication,
capacity, whole-workspace loss, and primary-Vault loss release gates.
