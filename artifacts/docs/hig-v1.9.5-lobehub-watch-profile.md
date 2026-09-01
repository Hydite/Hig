# Hig v1.9.5 LobeHub Watch Benchmark

- Environment: `ENVIRONMENT_NOT_QUALIFIED`
- Input: `15330` files / `198974614` bytes
- Corpus write: `58207.671 ms`
- Watcher bootstrap catch-up: `1027.735 ms`
- Single edit prepare: `33.953 ms`
- Five edit prepare: `31.346 ms`
- 1000-event burst catch-up: `175.721 ms`

| scenario | duration ms | archive bytes |
|---|---:|---:|
| normal first | 1832.767 | - |
| normal warm | 494.211 | - |
| project bootstrap pack | 2219.595 | - |
| project single edit pack | 344.799 | - |
| project five edit pack | 415.562 | - |
| project burst pack | 510.455 | 57111222 |
| project CLI wall | 342.197 | - |
| zip | 4703 | 67749383 |
| tar.gz | 14821 | 61354111 |
| tar.zst | 11190 | 64956374 |

- Project hash reuses: `15330`
- Prepared object hits: `879`
- Project metadata verify: `11541` us
- Watcher overflow count: `0`
- Correctness digest match: `true`

## Release Gates

- project warm <150ms: `false`
- project CLI <250ms: `false`
- single prepare <50ms: `true`
- five edit pack <150ms: `false`
- 1000-event burst <2s: `true`
- archive quality <= v1.8.5 +1%: `true`

## Project Warm Stage Breakdown

| stage | median us | p95 us |
|---|---:|---:|
| project_verify_us | 16509 | 69670 |
| plan_us | 4149 | 5559 |
| read_us | 0 | 0 |
| compression_us | 0 | 0 |
| crypto_us | 27152 | 58080 |
| manifest_serialize_us | 1948 | 2820 |
| manifest_compress_us | 3919 | 5237 |
| manifest_encrypt_us | 1075 | 1735 |
| output_create_us | 175 | 527 |
| output_write_us | 193784 | 390908 |
| output_flush_us | 116716 | 248581 |
| output_rename_us | 18923 | 50332 |
| cache_commit_us | 0 | 1 |
| unattributed_us | 17 | 22 |

## Project Warm Top Hotspots

- `output_write_us`: median `193784us`, p95 `390908us`
- `output_flush_us`: median `116716us`, p95 `248581us`
- `crypto_us`: median `27152us`, p95 `58080us`
