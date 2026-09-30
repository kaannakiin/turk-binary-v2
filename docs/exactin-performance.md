# ExactIn search performance comparison

This report compares the existing DFS search with the layered and flow
candidate searches on one fixed captured state. It is a representative
measurement, not a production SLA and not a proof of global route optimality.

## Reproduction

Input is the snapshot recorded in [exactin-baseline.md](exactin-baseline.md):
736 pools at slot `451259947`, SHA-256
`13971c60a4fd1ac9361ab9041b226980156834bf86c56f1ba0adc1b8e5fe534c`.

The benchmark uses fresh `SearchSession` instances for every engine and query,
so each result starts from the same captured state. The benchmark caps every
query at `25_000` quotes. It exercises `per_pair = all` and `per_pair = 3`.
Cycle queries compare DFS and layered search. Swap queries additionally
compare flow with `singleRouteOnly`, default split, and `singlePoolPerHop`.

```sh
ROUTE_UNIVERSE=/Users/kaanakin/Desktop/turk-binary-v2/oracle/snapshots/universe.json.gz \
CARGO_TARGET_DIR=/private/tmp/turk-binary-perf-target \
cargo bench -p route --bench search -- 'route/(sol_cycle_h2|sol_to_usdc_h2)' \
  --sample-size 10 --warm-up-time 1 --measurement-time 1 --noplot
```

The same command was run with `sol_cycle_h3`, `pump_cycle_h3`, and
`sol_to_usdc_h3` filters. `cargo bench` already uses the optimized bench
profile with this toolchain; `cargo bench --release` is rejected by Cargo
1.98.1.

## Quality before timing

The benchmark prints these values before Criterion timing. `delta` is the
absolute difference from the capped DFS result for that same query; it does
not mean that DFS is globally optimal when either result is exhausted.

| Query            | Mode                 | Pair cap |      Output |  Delta | Quotes | Exhausted | Legs |
| ---------------- | -------------------- | -------- | ----------: | -----: | -----: | --------- | ---: |
| `sol_to_usdc_h3` | DFS                  | all      | 118,348,564 |      0 |  1,060 | no        |    1 |
| `sol_to_usdc_h3` | layered              | all      | 118,348,564 |      0 |  1,060 | no        |    1 |
| `sol_to_usdc_h3` | flow single route    | all      | 118,348,564 |      0 |  1,060 | no        |    1 |
| `sol_to_usdc_h3` | flow split           | all      | 118,352,554 | +3,990 |  7,678 | no        |    3 |
| `sol_to_usdc_h3` | flow single pool/hop | all      | 118,348,564 |      0 |  7,525 | no        |    1 |
| `sol_to_usdc_h3` | DFS                  | 3        | 118,348,564 |      0 |    772 | no        |    1 |
| `sol_to_usdc_h3` | layered              | 3        | 118,348,564 |      0 |  1,060 | no        |    1 |
| `sol_to_usdc_h3` | flow single route    | 3        | 118,348,564 |      0 |    772 | no        |    1 |
| `sol_to_usdc_h3` | flow split           | 3        | 118,352,554 | +3,990 |  5,554 | no        |    3 |
| `sol_to_usdc_h3` | flow single pool/hop | 3        | 118,348,564 |      0 |  5,401 | no        |    1 |

The h2 swap produced the same outputs and the same split improvement. Cycle
quality was not equivalent under this cap: for example, `sol_cycle_h2/3`
returned `999,581,169` with DFS in 2,830 quotes, while layered returned
`999,396,469` after exhausting 25,000 quotes. `sol_cycle_h3/3` showed the same
`184,700` difference. Capped cycle runs therefore cannot support a quality
regression claim for layered search.

## Timing medians

Criterion used 10 samples, one second of warm-up, and one second of measured
time per benchmark. Values are medians from the reported low/median/high
intervals.

| Query                |       DFS |   Layered | Flow single route | Flow split | Flow single pool/hop |
| -------------------- | --------: | --------: | ----------------: | ---------: | -------------------: |
| `sol_cycle_h2/all`   | 33.257 ms | 30.585 ms |                 — |          — |                    — |
| `sol_cycle_h2/3`     | 3.9425 ms | 29.061 ms |                 — |          — |                    — |
| `sol_cycle_h3/all`   | 32.661 ms | 27.771 ms |                 — |          — |                    — |
| `sol_cycle_h3/3`     | 14.819 ms | 27.089 ms |                 — |          — |                    — |
| `pump_cycle_h3/all`  | 9.0634 ms | 22.975 ms |                 — |          — |                    — |
| `pump_cycle_h3/3`    | 6.0371 ms | 21.376 ms |                 — |          — |                    — |
| `sol_to_usdc_h2/all` | 989.99 µs | 945.66 µs |         944.86 µs |  14.831 ms |            5.3359 ms |
| `sol_to_usdc_h2/3`   | 952.10 µs | 1.0426 ms |         1.0928 ms |  14.335 ms |            4.5153 ms |
| `sol_to_usdc_h3/all` | 955.99 µs | 980.48 µs |         1.1422 ms |  13.919 ms |            4.7387 ms |
| `sol_to_usdc_h3/3`   | 788.49 µs | 942.54 µs |         825.98 µs |  12.201 ms |            4.2654 ms |

The split result is more valuable on the measured SOL→USDC case, but costs
roughly 13–15 ms under this candidate strategy. `singleRouteOnly` is close to
DFS. `singlePoolPerHop` preserves the single-pool output while paying for
candidate evaluation.

The 13–15 ms above predate the memo and the write-set and copy changes
recorded in [architecture.md](architecture.md); on the same capture the flow
split now takes 1.1–1.3 ms at 1 SOL (`just bench-ab`, 2026-09-30).

## Split across trade sizes

A split pays only once one pool's liquidity runs thin, which a 1 SOL order
never reaches. `cargo bench -p route --bench split` runs each swap from 1 to
10,000 SOL, with the same 25,000-quote cap and `max_arrays` 8, and compares
the current split with the chunked split (`FlowOptions::chunks`, see
[architecture.md](architecture.md)). Every flow of every engine was quoted
again with `requote_flow` in a fresh session and paid exactly its planned
output.

Gain over the single route in millionths of its output, with quotes computed
(memo answers excluded) in parentheses. `sol_to_usdc_h3` gave the same outputs
as `h2`.

| `sol_to_usdc_h2` |          split |       chunks 8 |      chunks 16 |      chunks 32 |
| ---------------- | -------------: | -------------: | -------------: | -------------: |
| 1 SOL            |     33 (2,977) |     37 (2,064) |     37 (2,071) |     37 (2,103) |
| 10 SOL           |      1 (2,817) |      1 (1,953) |      5 (1,985) |      6 (2,030) |
| 100 SOL          |     32 (2,644) |     32 (1,837) |     38 (1,892) |     40 (1,915) |
| 1,000 SOL        |  1,099 (2,574) |  1,358 (1,783) |  1,358 (1,810) |  1,373 (1,840) |
| 10,000 SOL       | 51,500 (2,507) | 56,520 (1,777) | 56,520 (1,767) | 56,649 (1,794) |

| `sol_to_pump_h2` |      split |        chunks 8 |       chunks 16 |       chunks 32 |
| ---------------- | ---------: | --------------: | --------------: | --------------: |
| 1 SOL            | 0 (10,835) |      0 (12,038) |      0 (12,118) |      0 (12,332) |
| 10 SOL           | 0 (10,569) |      0 (11,192) |      0 (11,344) |      0 (11,468) |
| 100 SOL          | 0 (10,206) |  1,021 (10,303) |  1,360 (10,894) |  1,360 (11,080) |
| 1,000 SOL        |  0 (9,561) |  26,244 (9,427) | 30,652 (10,094) | 31,307 (10,625) |
| 10,000 SOL       |  0 (9,219) | 112,945 (8,875) | 112,945 (9,043) | 129,512 (9,462) |

On the pump token the current split exhausts its 25,000 quote calls at every
size: a single path search there makes about 8,000, and discovery runs several.
The chunked split counts the budget in quotes computed and stays inside it.
The allocation polish after the chunks matters most where the greedy choice
is coarse: at 10,000 SOL with 8 chunks it lifts the pump gain from 102,712 to
112,945 millionths.

Timing medians (Criterion, 10 samples, 1 s):

| Query                   | single route |   split | chunks 8 | chunks 16 | chunks 32 |
| ----------------------- | -----------: | ------: | -------: | --------: | --------: |
| `sol_to_usdc_h2` 1      |      0.37 ms | 1.63 ms |  1.63 ms |   2.41 ms |   2.83 ms |
| `sol_to_usdc_h2` 1,000  |      0.78 ms | 2.21 ms |  2.01 ms |   2.49 ms |   3.57 ms |
| `sol_to_usdc_h2` 10,000 |      0.79 ms | 3.06 ms |  3.34 ms |   3.35 ms |   4.45 ms |
| `sol_to_pump_h2` 1      |      2.38 ms | 4.35 ms |  7.38 ms |  11.01 ms |  18.76 ms |
| `sol_to_pump_h2` 1,000  |      2.72 ms | 5.37 ms |  7.08 ms |  11.42 ms |  18.15 ms |
| `sol_to_pump_h2` 10,000 |      2.74 ms | 5.38 ms |  7.11 ms |   9.54 ms |  22.51 ms |

Each chunk walks the whole graph again; the memo makes that a lookup per edge
an earlier chunk did not move, but on the pump token's wide graph those
lookups (up to about 300,000 calls at 32 chunks) are what the time is.

## Correctness and interpretation limits

- All methods used the same snapshot, amount, goal, hop limit, pair cap, and
  quote cap for each comparison.
- Flow’s split output is intentionally a different search space from a single
  path, so its positive delta is not a regression test against DFS.
- Cycle runs with `exhausted=true` are incomplete searches. Their output is
  only the best route found before the quote cap.
- Layered currently needs the same path/flow admission hook forwarding as DFS
  before resource-budget comparisons are considered final. Rerun this report
  after that integration change.
- No global optimum claim is made for flow candidate pruning or allocation
  refinement.

## HTTP, allocation, and RSS measurement

[The HTTP report](exactin-http-performance.md) contains a release-mode
128-request matrix for all three endpoints, two route modes, and concurrency
1/2/4. It records p50/p95/p99, throughput, allocator counts/bytes, and a
separate sampled RSS replay with capture and toolchain metadata. Its allocator
numbers cover the HTTP request path; they do not isolate SDK quote, route
search, transaction assembly, and JSON serialization allocations. A separate
component profile is required before claiming an allocation optimization.
