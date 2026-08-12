# Hig v1.9.0 LobeHub Watch Benchmark

- Environment: `ENVIRONMENT_NOT_QUALIFIED`
- Input: `15330` files / `198974626` bytes
- Corpus write: `12762.489 ms`
- Watcher bootstrap catch-up: `1017.993 ms`
- Single edit prepare: `26.433 ms`
- Five edit prepare: `31.582 ms`
- 1000-event burst catch-up: `83.990 ms`

| scenario | duration ms | archive bytes |
|---|---:|---:|
| normal first | 887.404 | - |
| normal warm | 349.282 | - |
| project bootstrap pack | 946.602 | - |
| project single edit pack | 199.004 | - |
| project five edit pack | 138.629 | - |
| project burst pack | 145.480 | 57108402 |
| project CLI wall | 154.899 | - |
| zip | 4038 | 67749391 |
| tar.gz | 10116 | 61310145 |
| tar.zst | 6774 | 64899584 |

- Project hash reuses: `15330`
- Prepared object hits: `879`
- Project metadata verify: `10152` us
- Watcher overflow count: `0`
- Correctness digest match: `true`

## Release Gates

- project warm <150ms: `false`
- project CLI <250ms: `true`
- single prepare <50ms: `true`
- five edit pack <150ms: `true`
- 1000-event burst <2s: `true`
- archive quality <= v1.8.5 +1%: `true`
