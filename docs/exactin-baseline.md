# ExactIn route-search baseline

Baseline captured from commit `b8f2eafbe44532191f747f2febe14b796696cf74`
(`feat: Add Meteora and Orca adapters to router`). No production files were
changed for this baseline.

## Fixed inputs

- Snapshot: `oracle/snapshots/universe.json.gz`
- Snapshot SHA-256: `13971c60a4fd1ac9361ab9041b226980156834bf86c56f1ba0adc1b8e5fe534c`
- Snapshot size: 4,058,156 bytes
- Snapshot slot: `451259947`
- Pools: 736
- DEX distribution: Raydium AMM v4 17, Raydium CLMM 24, Raydium CPMM 14,
  Orca Whirlpool 14, Meteora DLMM 177, Meteora DAMM v2 353, Meteora DAMM v1
  18, Pump AMM 119
- Machine: Apple Silicon arm64, Darwin 24.6.0
- Toolchain: `rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1`
- Benchmark target: `/private/tmp/turk-binary-baseline-target`

Git-pinned SDK inputs from `Cargo.lock`:

- `kaannakiin/raydium-amm`: `e310ed8c438737f7c9bf7a56374ec53ae36d37c9`
- `kaannakiin/raydium-clmm`: `1de19c560b751cb685dea31e1aeb18f2f2602525`
- `kaannakiin/raydium-cp-swap`: `8055493659a014b7b0d00ff8d1391edba6792779`
- `kaannakiin/whirlpools`: `536d2dac6c53eb50da09b4534ac5113b5c5c7052`
- `kaannakiin/dlmm-sdk`: `28e1f83f053aa64a80e33b5a0c71d5f509e2384d`
- `kaannakiin/damm-v2`: `0506639d8137024301829854d545533c2b9d1ea5`

## Route benchmark

Command, from the detached baseline worktree:

```sh
ROUTE_UNIVERSE=/Users/kaanakin/Desktop/turk-binary-v2/oracle/snapshots/universe.json.gz \
CARGO_TARGET_DIR=/private/tmp/turk-binary-baseline-target \
cargo bench -p route --bench search -- \
  --sample-size 10 --warm-up-time 1 --measurement-time 1 --noplot
```

`cargo bench --release` is rejected by Cargo 1.98.1; `cargo bench` already
uses the optimized bench profile. The values below are Criterion's
`[low median high]` intervals. They exclude compilation and benchmark
startup. The built-in corpus has five queries and each query is measured with
`per_pair = all, 1, 2, 3`.

| Query | all | per-pair 1 | per-pair 2 | per-pair 3 |
| --- | ---: | ---: | ---: | ---: |
| `sol_cycle_h2` | `[622.63, 686.01, 756.64] ms` | `[1.6391, 1.8443, 2.0501] ms` | `[3.2941, 4.3712, 5.3168] ms` | `[4.1272, 4.7602, 6.2493] ms` |
| `sol_cycle_h3` | `[549.59, 712.71, 905.82] ms` | `[2.3941, 2.5161, 2.6618] ms` | `[6.2538, 7.2343, 7.8910] ms` | `[10.529, 11.293, 12.002] ms` |
| `sol_to_usdc_h2` | `[757.42, 825.19, 867.14] µs` | `[743.25, 789.09, 866.96] µs` | `[698.69, 735.56, 792.85] µs` | `[696.64, 730.45, 813.58] µs` |
| `sol_to_usdc_h3` | `[801.23, 863.50, 995.68] µs` | `[702.46, 763.11, 884.42] µs` | `[669.26, 693.88, 743.74] µs` | `[800.09, 842.24, 901.06] µs` |
| `pump_cycle_h3` | `[165.56, 173.83, 183.70] ms` | `[2.0273, 2.3329, 2.7708] ms` | `[3.0961, 3.2971, 3.5679] ms` | `[5.0609, 5.3501, 5.8463] ms` |

Criterion reported mild/severe outliers in a few 10-sample groups. They are
retained in the raw command output and are not hidden by this summary.

## Independent quality check

Command:

```sh
ROUTE_UNIVERSE=/Users/kaanakin/Desktop/turk-binary-v2/oracle/snapshots/universe.json.gz \
CARGO_TARGET_DIR=/private/tmp/turk-binary-baseline-quality-target \
cargo nextest run --release -p route --test snapshot --run-ignored only --no-capture
```

Result: **pass**. Test
`pruning_by_max_hops_matches_the_exhaustive_search` passed; 1 passed, 0
failed, 0 ignored, 0 filtered; test execution `18.25s`. The test exercised
the full built-in query corpus and its deterministic random amount set. It
reported no pruned-vs-exhaustive route or output mismatches.

## Scope and gaps

This is a route-search baseline only. It does not measure HTTP parsing,
split/merge planning, transaction encoding, allocation counts, peak memory,
or concurrent request throughput. The benchmark is a representative short
Criterion run (`sample_size=10`, one-second warm-up and measurement per
group), so later comparisons must use the same command and fixed snapshot
before expanding the sample size.
