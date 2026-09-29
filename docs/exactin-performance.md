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
