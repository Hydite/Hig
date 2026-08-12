# Hig v1.9.1 LobeHub Watch Benchmark

- Environment: `ENVIRONMENT_NOT_QUALIFIED`
- Input: `15330` files / `198974618` bytes
- Corpus write: `12888.032 ms`
- Watcher bootstrap catch-up: `1035.417 ms`
- Single edit prepare: `28.989 ms`
- Five edit prepare: `31.070 ms`
- 1000-event burst catch-up: `111.635 ms`

| scenario | duration ms | archive bytes |
|---|---:|---:|
| normal first | 978.957 | - |
| normal warm | 562.078 | - |
| project bootstrap pack | 842.456 | - |
| project single edit pack | 253.535 | - |
| project five edit pack | 96.243 | - |
| project burst pack | 120.430 | 57108395 |
| project CLI wall | 164.008 | - |
| zip | 4088 | 67749381 |
| tar.gz | 10098 | 61313475 |
| tar.zst | 6724 | 64898790 |

- Project hash reuses: `15330`
- Prepared object hits: `879`
- Project metadata verify: `10647` us
- Watcher overflow count: `0`
- Correctness digest match: `true`

## Release Gates

- project warm <150ms: `true`
- project CLI <250ms: `true`
- single prepare <50ms: `true`
- five edit pack <150ms: `true`
- 1000-event burst <2s: `true`
- archive quality <= v1.8.5 +1%: `true`

## Project Warm Stage Breakdown

| stage | median us | p95 us |
|---|---:|---:|
| project_verify_us | 10102 | 16018 |
| plan_us | 2639 | 3242 |
| read_us | 0 | 0 |
| compression_us | 0 | 0 |
| crypto_us | 17104 | 27780 |
| manifest_serialize_us | 1004 | 1110 |
| manifest_compress_us | 2125 | 2352 |
| manifest_encrypt_us | 690 | 1237 |
| output_create_us | 119 | 312558 |
| output_write_us | 52103 | 396781 |
| output_flush_us | 15349 | 61774 |
| output_rename_us | 6982 | 19151 |
| cache_commit_us | 0 | 0 |
| unattributed_us | 14 | 23 |

## Project Warm Top Hotspots

- `output_write_us`: median `52103us`, p95 `396781us`
- `crypto_us`: median `17104us`, p95 `27780us`
- `output_flush_us`: median `15349us`, p95 `61774us`
