# HIG Golden Fixtures

These fixtures are immutable, synthetic compatibility evidence. They contain
no private project data, user path, production password, cache state, or paper
material.

## Archive Fixtures

`archives/v1.9.6` was generated with the macOS universal CLI from the published
HIG v1.9.6 IDE/MCP package. Historical CLI SHA-256:

```text
2d009e300dc4c361509f267fd556dbd50991613208c8237ff34c8d0f4ed69025
```

It includes HIGV1 password, HIGV2 legacy-manifest password, HIGV2 compact-
manifest password, and HIGV2 compact no-encryption archives. The public test
password is `hig-public-fixture-v1`.

## Repository Fixture

`repositories/v1.10.0-direct-head` was generated from public release commit
`272a1e87f5211f5ccc5f70b881aee84926cc4806`, before explicit branch and tag
references were added. Historical CLI SHA-256:

```text
181daebc998675080df24bec82f7c875336d555a95e114e82b9ee27c3593966e
```

The repository has two commits and only the legacy `refs/HEAD` reference. The
current reader must verify and restore it before migration, then migrate it to
symbolic `HEAD` plus `refs/heads/main` without rewriting an object.

## Policy

Files under this directory must never be replaced. A format or schema release
adds a new version directory, its source files, its `SHA256SUMS`, and provenance
to this document. `scripts/verify-golden-fixtures.sh` is a mandatory release
gate. `scripts/generate-golden-fixtures.sh` exists for audited additions and
refuses to overwrite an existing fixture directory.
