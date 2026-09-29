# ExactIn HTTP performance measurements

The ignored server test `http_endpoints_report_release_latency_matrix` measures
the ExactIn HTTP path against the checked-in full universe capture:

- capture: `oracle/snapshots/universe.json.gz`
- capture slot: `451259947`
- capture size: `736` pools across `8` DEXes
- endpoints: `/quote`, `/swap-instructions`, `/swap`
- route modes: `singleRouteOnly=true` and split allowed (`singleRouteOnly=false`)
- concurrency: `1`, configured workers, and `2 * configured workers`

Run it in release mode so debug checks and code generation do not shape the
latency numbers:

```sh
HTTP_PERF_WORKERS=8 \
HTTP_PERF_MAX_QUEUED=128 \
HTTP_PERF_TIMEOUT_MS=10000 \
HTTP_PERF_SAMPLES=64 \
HTTP_PERF_WARMUP=8 \
HTTP_PERF_RSS_INTERVAL_MS=10 \
cargo test -p server --release http_endpoints_report_release_latency_matrix -- \
  --ignored --nocapture
```

The test prints one `http_perf_meta=...` JSON object followed by one
`http_perf=...` object for every endpoint, route mode, and concurrency pair.
Each measurement includes p50/p95/p99 latency in microseconds, elapsed time,
throughput, status counts, allocation/deallocation counts and bytes from the
`stats_alloc` test allocator, and RSS before, after, and the maximum observed
by a sampler during a replay pass. The RSS pass is separate from the latency
and allocation pass so process sampling does not contaminate those numbers.

The test has no timing assertion and remains ignored in CI. Store the complete
`http_perf_meta` line with the output because it identifies the corpus hash,
Cargo.lock hash, commit, compiler, machine, worker count, sample count, and
RSS sampling interval. Compare runs only when those inputs match.
The queue and timeout values are included in the metadata; increasing them
prevents the benchmark harness from turning a long search into an overload
response, while preserving the measured request latency.

## Diagnostic run

The following release run used `HTTP_PERF_WORKERS=2`, four measured
requests, one warmup request, and a 5 ms RSS sampling interval on 2026-09-29.
It is a smoke measurement for the harness, not a stable performance baseline.

| endpoint             | mode          | concurrency | p50 / p95 / p99 (µs) |   req/s | allocations | allocated bytes | observed peak RSS (KiB) |
| -------------------- | ------------- | ----------: | -------------------: | ------: | ----------: | --------------: | ----------------------: |
| `/quote`             | single-route  |           1 |      539 / 872 / 872 | 1533.67 |       8,314 |       6,293,568 |                  75,920 |
| `/quote`             | single-route  |           2 |      714 / 931 / 931 | 2391.87 |       8,311 |       6,293,296 |                  76,432 |
| `/quote`             | single-route  |           4 |    700 / 1235 / 1235 | 3117.29 |       8,310 |       6,293,224 |                  76,432 |
| `/swap-instructions` | single-route  |           1 |      446 / 510 / 510 | 2078.15 |       7,022 |       6,014,132 |                  76,624 |
| `/swap-instructions` | single-route  |           2 |      515 / 615 / 615 | 3471.47 |       7,019 |       6,013,236 |                  76,656 |
| `/swap-instructions` | single-route  |           4 |    633 / 1084 / 1084 | 3579.68 |       7,018 |       6,013,164 |                  76,656 |
| `/swap`              | single-route  |           1 |      420 / 510 / 510 | 2152.27 |       6,353 |       5,936,256 |                  76,656 |
| `/swap`              | single-route  |           2 |      516 / 550 / 550 | 3593.08 |       6,351 |       5,936,112 |                  76,656 |
| `/swap`              | single-route  |           4 |    580 / 1002 / 1002 | 3825.61 |       6,350 |       5,936,040 |                  76,736 |
| `/quote`             | split-allowed |           1 |   3273 / 3575 / 3575 |  295.16 |      49,733 |      46,599,568 |                  76,800 |
| `/quote`             | split-allowed |           2 |   3603 / 3658 / 3658 |  548.74 |      49,732 |      46,600,176 |                  76,816 |
| `/quote`             | split-allowed |           4 |   3683 / 7221 / 7221 |  551.07 |      49,730 |      46,599,352 |                  76,848 |
| `/swap-instructions` | split-allowed |           1 |   2588 / 2705 / 2705 |  379.24 |      40,105 |      59,309,400 |                  76,848 |
| `/swap-instructions` | split-allowed |           2 |   2957 / 3173 / 3173 |  651.40 |      40,103 |      59,309,256 |                  77,024 |
| `/swap-instructions` | split-allowed |           4 |   2992 / 5715 / 5715 |  696.16 |      40,102 |      59,309,184 |                  77,056 |
| `/swap`              | split-allowed |           1 |   2670 / 2747 / 2747 |  367.99 |      38,517 |      59,089,152 |                  77,056 |
| `/swap`              | split-allowed |           2 |   2820 / 3000 / 3000 |  682.28 |      38,515 |      59,089,008 |                  77,056 |
| `/swap`              | split-allowed |           4 |   2864 / 5576 / 5576 |  711.12 |      38,515 |      59,089,688 |                  77,056 |

The run used corpus hash `f7ac688338d1e8a1b29468479bc98399def97bfb`, Cargo.lock
hash `ae71b4ebc0bff71684ed3c31bf4cd83ae328402b`, and commit
`b8f2eafbe44532191f747f2febe14b796696cf74`. All 18 combinations returned HTTP 200. Split-allowed search currently explores more candidates, so its higher
latency and allocation totals are expected observations for this corpus.

## 128-request release run

On 2026-09-29, the same corpus was replayed with 128 measured requests, 8
warmups, 2 workers, and concurrency 1/2/4. Every request and every 64-request
RSS replay returned HTTP 200. The table reports allocations and bytes across
the 128 measured requests, not per request. Peak RSS is the largest sample in
the separate RSS replay. The run used commit `76a024e2a85e07535e3afb67685655d1c855f8df`
with uncommitted ExactIn changes, Rust 1.98.1, Darwin arm64, capture slot
451259947 and corpus hash `f7ac688338d1e8a1b29468479bc98399def97bfb`.
`Cargo.lock` SHA-1 was `ae71b4ebc0bff71684ed3c31bf4cd83ae328402b`.

| Endpoint | Mode | C | p50 / p95 / p99 (µs) | req/s | Allocations | Allocated bytes | Peak RSS (KiB) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `/quote` | single | 1 | 473 / 594 / 844 | 1948.57 | 265,989 | 201,393,088 | 83,184 |
| `/quote` | single | 2 | 563 / 646 / 711 | 3338.27 | 265,925 | 201,388,480 | 94,944 |
| `/quote` | single | 4 | 923 / 1230 / 1471 | 3400.76 | 265,893 | 201,386,176 | 90,704 |
| `/swap-instructions` | single | 1 | 433 / 498 / 617 | 2182.35 | 224,645 | 192,431,168 | 97,616 |
| `/swap-instructions` | single | 2 | 502 / 548 / 593 | 3779.23 | 224,581 | 192,426,560 | 101,488 |
| `/swap-instructions` | single | 4 | 869 / 1025 / 1144 | 3893.88 | 224,549 | 192,424,256 | 109,584 |
| `/swap` | single | 1 | 433 / 521 / 861 | 2128.16 | 203,269 | 189,963,200 | 102,896 |
| `/swap` | single | 2 | 526 / 1109 / 1482 | 3063.40 | 203,205 | 189,958,592 | 109,664 |
| `/swap` | single | 4 | 879 / 1393 / 1500 | 3506.53 | 203,173 | 189,956,288 | 100,448 |
| `/quote` | split allowed | 1 | 3166 / 3447 / 3495 | 310.73 | 1,591,429 | 1,491,189,184 | 118,528 |
| `/quote` | split allowed | 2 | 3686 / 3870 / 3984 | 531.95 | 1,591,365 | 1,491,184,576 | 116,128 |
| `/quote` | split allowed | 4 | 4009 / 7552 / 7704 | 538.12 | 1,591,333 | 1,491,182,272 | 112,624 |
| `/swap-instructions` | split allowed | 1 | 2704 / 3236 / 3493 | 347.13 | 1,283,333 | 1,897,903,808 | 118,240 |
| `/swap-instructions` | split allowed | 2 | 3191 / 3449 / 3552 | 608.49 | 1,283,269 | 1,897,899,200 | 100,320 |
| `/swap-instructions` | split allowed | 4 | 3397 / 5877 / 6081 | 701.15 | 1,283,237 | 1,897,896,896 | 97,968 |
| `/swap` | split allowed | 1 | 2937 / 3100 / 3141 | 353.85 | 1,232,517 | 1,890,855,872 | 91,472 |
| `/swap` | split allowed | 2 | 3200 / 3289 / 3353 | 616.55 | 1,232,453 | 1,890,851,264 | 109,424 |
| `/swap` | split allowed | 4 | 3402 / 6604 / 6677 | 626.26 | 1,232,421 | 1,890,848,960 | 97,504 |

The four-request diagnostic run above is too small to establish a speedup or
regression. The split-allowed mode has materially higher measured cost than
single-route mode in this corpus. This run is an implementation baseline, not
evidence of an optimization gain.
