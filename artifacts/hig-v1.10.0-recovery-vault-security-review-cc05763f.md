# HIG v1.10.0 Recovery Vault Security Review Evidence

## Scope

- Candidate commit: `cc05763fe3e13734327acb4cfc630776cde6884b`
- CodeQL workflow: [33498060294](https://github.com/Hydite/Hig/actions/runs/33498060294)
- Quality workflow: [33498059314](https://github.com/Hydite/Hig/actions/runs/33498059314)
- Query suite: `extended`
- Languages: GitHub Actions, JavaScript/TypeScript, and Rust
- Result: all three analyzers completed successfully; zero alerts remain open.

This review covers the Recovery Vault implementation, repository and archive
storage paths used by recovery, the CLI, the MCP process boundary, packaging
and qualification scripts, and the GitHub Actions release boundary. It does not
claim that static analysis proves semantic correctness; deterministic fault,
loss, package, and native soak evidence remains independently required.

## Findings Disposition

The first extended analysis produced 425 alerts. The final candidate closed 27
alerts through code changes:

- 17 mutable third-party Action references were pinned to immutable commits;
- 5 release-script dynamic property writes gained flag allowlists and
  prototype-free argument maps;
- 2 package verification check/read races now use one open file descriptor;
- 1 package command data-flow alert closed with the release-script hardening;
- 1 package path data-flow alert closed with descriptor-based verification;
- 1 MCP resource-exhaustion alert closed after absolute resource limits were
  added and covered by the adversarial MCP test.

The remaining 398 alerts were reviewed by rule and sink:

| Disposition | Count | Basis |
|---|---:|---|
| False positive | 390 | The query did not model local CLI authority, argv-array process creation with `shell` disabled, serde JSON escaping, cryptographic buffer initialization, or the MCP physical-path sanitizers and child-side root capability. |
| Accepted residual risk | 8 | Caller-selected pathological local files may exhaust the invoking HIG process during large in-memory batch operations. Archive decoding remains hard-limited, and integrity and path confinement are unaffected. |

Every dismissal is recorded in GitHub with a rule-specific rationale. The final
API query for `state=open` returned an empty array after the exact candidate
commit completed all three extended analyzers.

## Verified Boundaries

- MCP paths are resolved through existing physical ancestors, confined to
  configured roots, revalidated immediately before spawn, and checked again by
  the HIG child through `HIG_MCP_ENFORCED_ROOTS`.
- MCP subprocesses use `spawn(executable, argv)` without shell interpolation.
- Passwords are supplied through stdin or in-process cryptographic calls and do
  not appear in report structs, argv, environment, or structured output.
- Recovery custody export and import remain CLI-only and are not exposed as MCP
  tools.
- Archive-relative paths reject absolute paths and parent traversal before
  publication.
- Recovery restore defaults to no overwrite; GC defaults to report-only.
- JavaScript dependencies pass the official npm vulnerability audit at
  `moderate` severity and above.
- Rust dependencies pass RustSec in the required Quality Gates workflow.
- Native watcher-backend restart retains the same repository root and policy,
  performs an authoritative reconciliation, and grants no new path or process
  capability.

## Residual Availability Risk

The eight accepted `rust/uncontrolled-allocation-size` findings are not remote
allocation primitives. They arise from local files or already bounded storage
collections selected by the invoking operator. A sufficiently large local file
can still cause the active HIG process to exhaust memory in cold scan or batch
preparation. This is an availability limitation, not a recovery-integrity,
confidentiality, authentication, or confinement bypass.

The next large-file reliability track should stream whole-file hashing and
chunk planning with a bounded working set while preserving archive format,
chunk identity, compression quality, and exact restore behavior. Until that
work is qualified, operators should retain the existing workspace resource
policy and run untrusted or exceptionally large inputs under an OS memory
limit.

## Review Conclusion

No unresolved CodeQL alert blocks the v1.10.0 Recovery Vault candidate. The
accepted large-local-file availability risk is documented and does not weaken
the deletion-loss recovery guarantee. Final release completion still depends
on the native two-hour macOS, Linux, and Windows soak reports for this exact
candidate commit.
