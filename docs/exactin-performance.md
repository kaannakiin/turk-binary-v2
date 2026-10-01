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
output; `just test-universe` asserts this at 1, 100 and 10,000 SOL.

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

The two budgets are not the same: the current split counts quote calls and
the chunked one quotes computed. On the pump token the current split exhausts
its 25,000 calls at every size (a single path search there makes about 8,000,
and discovery runs several). Given ten times the calls it computes about as
many quotes as the chunked split, and most of the gap closes:

| `sol_to_pump_h2` | split 25k calls | split 250k calls |        chunks 8 |
| ---------------- | --------------: | ---------------: | --------------: |
| 100 SOL          |      0 (10,206) |   1,022 (13,476) |  1,021 (10,303) |
| 1,000 SOL        |       0 (9,561) |  26,244 (11,834) |  26,244 (9,427) |
| 10,000 SOL       |       0 (9,219) |   7,993 (11,173) | 112,945 (8,875) |

So at 100 and 1,000 SOL the chunked split's gain on the pump token came from
its budget, not its method; at 10,000 SOL it is the method. On SOL→USDC the
current split is not budget-bound (the wider budget changes nothing), and the
chunked split's gain over it is its own: 0.48–0.49% more output at 10,000
SOL. Under the same 5 ms deadline and no quote budget, neither splits the pump
token at any size (both return the single route) and both finish SOL→USDC
unchanged. The allocation polish after the chunks matters most where the
greedy choice is coarse: at 10,000 SOL with 8 chunks it lifts the pump gain
from 102,712 to 112,945 millionths.

Timing medians (Criterion, 10 samples, 1 s):

| Query                   | single route |   split | split 250k | chunks 8 | chunks 16 | chunks 32 |
| ----------------------- | -----------: | ------: | ---------: | -------: | --------: | --------: |
| `sol_to_usdc_h2` 1      |      0.41 ms | 1.48 ms |    1.53 ms |  1.89 ms |   2.32 ms |   3.10 ms |
| `sol_to_usdc_h2` 1,000  |      0.72 ms | 2.34 ms |    2.27 ms |  2.13 ms |   2.16 ms |   2.96 ms |
| `sol_to_usdc_h2` 10,000 |      0.83 ms | 3.32 ms |    3.19 ms |  3.53 ms |   3.96 ms |   4.42 ms |
| `sol_to_pump_h2` 1      |      2.91 ms | 5.06 ms |   11.50 ms |  7.32 ms |  10.80 ms |  16.37 ms |
| `sol_to_pump_h2` 1,000  |      2.70 ms | 5.43 ms |    7.60 ms |  6.99 ms |  10.71 ms |  17.38 ms |
| `sol_to_pump_h2` 10,000 |      2.72 ms | 5.83 ms |    8.01 ms |  7.25 ms |   9.56 ms |  24.00 ms |

Each chunk walks the whole graph again; the memo makes that a lookup per edge
an earlier chunk did not move, but on the pump token's wide graph those
lookups (up to about 300,000 calls at 32 chunks) are what the time is. Chunks
8 is the candidate to try in the server first; 32 costs about three times its
time for about 1.5% more output on the largest pump order.

These plans are quote plans under `Everything`: none has been through the
server's transaction admission (`Swappable`) or executed in LiteSVM yet.

## Chunked split under admission

The figures above were taken with `Everything`: no transaction was built. The
same orders were quoted through the server's `/quote`, which admits every
candidate as a v1 transaction for a stand-in wallet (2026-10-01, the slot
451,259,947 capture, `per_pair` 2, `max_arrays` 8, `maxHops` 2). Output in base
units of the output mint; in parentheses the p50 in µs of nine requests, one
run, so a guide rather than a result:

| SOL → USDC | single route |              split |           chunks 8 |
| ---------- | -----------: | -----------------: | -----------------: |
| 1          |  118,348,564 | 118,351,590 (1055) | 118,350,425 (1001) |
| 1,000      |      118.08B |     118.21B (2195) |     118.23B (1890) |
| 10,000     |    1,110.38B |   1,154.53B (2889) |   1,171.08B (3044) |

| SOL → pump | single route |            split |           chunks 8 |
| ---------- | -----------: | ---------------: | -----------------: |
| 10         |       15.97B |    15.99B (1394) |      16.00B (1308) |
| 100        |      158.60B |   158.84B (1863) |     158.84B (1488) |
| 1,000      |    1,392.42B | 1,392.42B (2242) |   1,392.42B (3177) |
| 10,000     |       18.43B |    32.62B (6462) | 2,029.65B (15,525) |

Chunks 16 gained nothing over 8 and cost up to three times as long. A polish
step of 1 in 10,000 closed all but one loss to the current split (SOL→USDC at
1 SOL, 10 ppm). At 1,000 SOL to the pump token every split is refused: of the
plans admission saw, 1,956 per six requests exceeded the 1,400,000 compute
units of a v1 transaction; the per-hop budgets of `crates/tx/src/budget.rs`
allow two heavy hops at most. Admission now prices compute from the windows
before building anything, which took that query from about 4.6 to 3 ms.

**Execution.** `just router-split-replay` sends the orders the server splits
through the router in `LiteSVM`, on a capture's accounts and mainnet bytecode.
On a capture of slot 452,267,679, the current split's three plans paid exactly
what they quoted (SOL→USDC 10,000 SOL in 406,975 compute units, SOL→pump 10 SOL
in 124,563, 10,000 SOL in 524,586). Of the chunked plans for the same orders,
SOL→pump 10 SOL paid exactly, in 249,584 units; SOL→USDC 10,000 SOL and SOL→pump
10,000 SOL ran out of compute at the 1,320,000 and 1,400,000 units their
budgets requested. Both send thousands of SOL into a thin pool once the deep
ones are spent at the margin (3,080 SOL into CLMM `3ucNos…`, 1,490 SOL into DLMM
`qhJ7kL…`), and a swap that large crosses many ticks or bins inside the arrays
its budget counts. That was the budget, which counted arrays. Budgeted by the walk each quote
reports (initialized ticks or bins crossed and fee-loop steps,
`crates/tx/src/budget.rs`), with DLMM pools whose oracle the router refuses left
out, the same capture's plans all paid exactly what they quoted in their v1
transaction:

| Order               |        current split |             chunks 8 |
| ------------------- | -------------------: | -------------------: |
| SOL→USDC 10,000 SOL | 1,162.56B (1.04M CU) | 1,164.08B (1.05M CU) |
| SOL→pump 10 SOL     |    19.06B (0.12M CU) |    19.06B (0.25M CU) |
| SOL→pump 10,000 SOL |    37.24B (0.52M CU) | 1,742.96B (1.09M CU) |

On the earlier capture, whose CLMM pools lack the
observation account a swap now names, only the plans without CLMM ran; the
chunked SOL→pump 10,000 SOL plan there paid exactly, in 1,251,596 units.

The replay also swaps each plan's pools one by one outside the router; for
DLMM pools other than `5rCf1D…` that direct swap fails with `InvalidBinArray`,
a limit of the direct swap the oracle builds, not of the router.

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
