# Configuration

Config is one TOML file (`--config`, default `config.toml`). Start from [`config.example.toml`](../config.example.toml). Unknown keys are an error, so typos fail fast.

## Environment variables

Endpoints often carry API keys, so they live in the environment, never in the file.

| Variable          | Required | Meaning                                         |
| ----------------- | -------- | ----------------------------------------------- |
| `TB_RPC_URL`      | yes      | JSON-RPC endpoint                               |
| `TB_GRPC_URL`     | yes      | Yellowstone gRPC endpoint (`https://` uses TLS) |
| `TB_GRPC_X_TOKEN` | no       | Yellowstone `x-token`                           |
| `RUST_LOG`        | no       | Log filter, default `info`                      |

## `[universe]`: which pools to watch

| Key             | Default | Meaning                                                                    |
| --------------- | ------- | -------------------------------------------------------------------------- |
| `mints`         | `[]`    | Token mints. Every pool whose **both** tokens are in this list is watched. |
| `pools`         | `[]`    | Pool addresses to watch no matter what `mints` says.                       |
| `allowed_dexes` | `[]`    | Only these DEXes. Empty means every verified DEX.                          |
| `blocked_dexes` | `[]`    | Never these DEXes.                                                         |

How it is resolved:

1. **Enabled DEXes** = `allowed_dexes` (or all verified ones if empty), minus `blocked_dexes`.
2. **From `mints`**: on each enabled DEX, find pools where both sides are in `mints`. Three mints `A, B, C` give you every `A/B`, `A/C` and `B/C` pool. This uses one `getProgramAccounts` per ordered mint pair and DEX, so `n` mints cost `n × (n − 1)` calls per DEX. Many RPC providers block `getProgramAccounts` (HTTP 403); if yours does, `watch` stops and names the DEX. Use a provider that allows it, or list the pools under `pools`, which only needs `getMultipleAccounts`. Measured 2026-09-24: Shyft's free tier refuses it for every DEX program, Helius's free tier allows it.
3. **From `pools`**: each address is looked up and its DEX detected. These are added on top of step 2.

These are errors, not warnings:

- A DEX in both `allowed_dexes` and `blocked_dexes`.
- A DEX in `allowed_dexes` that is not verified yet (see [dexes.md](dexes.md)).
- Nothing left enabled after blocking.
- Both `mints` and `pools` empty, or `mints` with a single entry.
- A pool in `pools` that doesn't exist, isn't a supported DEX pool, or belongs to a disabled DEX.

## Commitment values

`commitment` accepts `processed`, `confirmed` or `finalized` (any case).

- **`processed`** (default): newest state, about one slot behind the leader. Needed for arbitrage, since `confirmed` data is already stale by the time you act on it.
- **`confirmed`** / **`finalized`**: safer against forks, but too slow for arbitrage.

`processed` can show a block that is later dropped. That is handled by fork tracking (see [architecture.md](architecture.md#fork-tracking)): unconfirmed updates are kept apart and rolled back if their fork loses.

## `[rpc]`

| Key                   | Default       | Meaning                                                                                                                                             |
| --------------------- | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `commitment`          | `"processed"` | Commitment for universe resolution. Seeds and repairs always read at `confirmed`.                                                                   |
| `timeout_ms`          | `10000`       | Per-request timeout                                                                                                                                 |
| `max_in_flight`       | `8`           | Max concurrent RPC requests                                                                                                                         |
| `max_rps`             | `8`           | Ceiling on requests per second, paced across every method; each 429 lowers the pace below it, successes raise it back slowly. `0` turns pacing off. |
| `retry.max_attempts`  | `5`           | Attempts for timeouts, 429s, 5xx and "node behind" errors. Other errors fail immediately.                                                           |
| `retry.base_delay_ms` | `100`         | First backoff; doubles each attempt                                                                                                                 |
| `retry.max_delay_ms`  | `5000`        | Backoff cap                                                                                                                                         |

## `[grpc]`

### Streams

| Key                          | Default       | Meaning                                                                                                                                       |
| ---------------------------- | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `commitment`                 | `"processed"` | See [Commitment values](#commitment-values)                                                                                                   |
| `streams`                    | `12`          | Pool shards. A pool and its pool-only dependencies share one; shared accounts use one extra stream per partition. Empty shards never connect. |
| `slot_source`                | `"auto"`      | `slots`, `blocks_meta` or `auto`. See [architecture.md](architecture.md#fork-tracking).                                                       |
| `max_pubkeys_per_filter`     | `100`         | Longer address lists are split into several filters. Lowered automatically if the server reports a limit.                                     |
| `max_account_filters`        | unset         | Max account filters per request. Unset means learn it from the server's error.                                                                |
| `max_txn_pubkeys_per_filter` | `100`         | The same split for the transaction-status filters on shards. Lowered automatically if the server reports a limit.                             |
| `max_txn_filters`            | unset         | Max transaction-status filters per request. Unset means learn it from the server's error.                                                     |
| `warn_request_bytes`         | `2097152`     | Log a warning when a subscribe request is larger                                                                                              |
| `max_request_bytes`          | `4000000`     | Refuse groups that would make a request larger                                                                                                |
| `compression`                | `"gzip"`      | `none`, `gzip` or `zstd`. Use `none` when the gRPC node is on the same machine.                                                               |
| `recv_timeout_ms`            | `10000`       | Reconnect a stream that has received nothing for this long. `0` turns it off.                                                                 |
| `max_message_delay_ms`       | `10000`       | Reconnect a stream whose messages are older than this (server `created_at`). `0` turns it off.                                                |
| `connect_timeout_ms`         | `10000`       | Connect timeout                                                                                                                               |
| `max_message_bytes`          | `67108864`    | Largest accepted message                                                                                                                      |
| `event_buffer`               | `16384`       | Events queued between the streams and each partition's engine                                                                                 |
| `filter_flush_ms`            | `200`         | Subscription changes are collected this long and sent as one filter update                                                                    |
| `filter_ack_timeout_ms`      | `2000`        | If no update confirms a filter change in this time, it is assumed effective at the latest slot                                                |

Every stream also subscribes to the Clock sysvar, which updates every slot. The receive timer resets on every message, so a pool with no trades never trips it.

### Reconnects

| Key                       | Default | Meaning                                                               |
| ------------------------- | ------- | --------------------------------------------------------------------- |
| `reconnect.max_attempts`  | `0`     | Failed reconnects in a row before `watch` exits. `0` retries forever. |
| `reconnect.base_delay_ms` | `500`   | First reconnect delay                                                 |
| `reconnect.max_delay_ms`  | `30000` | Reconnect delay cap                                                   |

After a reconnect, only that stream's accounts are read again over RPC (see [architecture.md](architecture.md#reconnects)).

**Removed keys.** `recover_missed_data`, `slot_retention`, `stream_reconnect_attempts`, `stream_reconnect_base_ms` and `command_buffer` no longer exist. Unknown keys are an error, so delete them from older configs.

### `[grpc.transport]` (HTTP/2 and TCP)

| Key                              | Default    | Meaning                               |
| -------------------------------- | ---------- | ------------------------------------- |
| `http2_adaptive_window`          | `true`     | Adaptive HTTP/2 flow control          |
| `http2_keep_alive_interval_ms`   | `15000`    | HTTP/2 keep-alive ping interval       |
| `keep_alive_timeout_ms`          | `5000`     | Keep-alive ping response timeout      |
| `keep_alive_while_idle`          | `true`     | Keep pinging when there is no traffic |
| `tcp_keepalive_ms`               | `30000`    | OS TCP keepalive. `0` turns it off.   |
| `tcp_nodelay`                    | `true`     | Disable Nagle's algorithm             |
| `initial_connection_window_size` | `16777216` | HTTP/2 connection window (bytes)      |
| `initial_stream_window_size`     | unset      | HTTP/2 stream window (bytes)          |
| `buffer_size`                    | `8192`     | Client request buffer                 |

## `[sync]`

| Key                          | Default | Meaning                                                                                                                 |
| ---------------------------- | ------- | ----------------------------------------------------------------------------------------------------------------------- |
| `settle_slots`               | `4`     | A seed read must be at least this many slots after the slot its filter became effective                                 |
| `repair_concurrency`         | `2`     | Seed and repair reads in flight at once                                                                                 |
| `repair_batch`               | `100`   | Keys per read (`getMultipleAccounts` takes at most 100)                                                                 |
| `repair_retry.base_delay_ms` | `500`   | First delay before a failed read is retried                                                                             |
| `repair_retry.max_delay_ms`  | `30000` | Retry delay cap. Failed reads are retried until they succeed.                                                           |
| `audit_interval_ms`          | `10000` | One drift check of up to 100 accounts per interval. `0` turns it off.                                                   |
| `stream_swap_accounts`       | `true`  | Also subscribe accounts only the swap instruction needs (vaults the math does not read, DLMM oracle)                    |
| `tick_ms`                    | `50`    | How often closures, reads and readiness are brought up to date                                                          |
| `txn_max_hold_ms`            | `5000`  | Safety cap: a shard's transaction writes held this long are applied and their pools re-read before they are ready again |

## `[route]`

| Key            | Default   | Meaning                                                                                                                                                    |
| -------------- | --------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `probe_amount` | `1000000` | `watch` quotes this raw input amount both ways through every decoded pool eaech stats tick and logs the outcomes per DEX (`quote probe`); `0` turns it off |

## `[threads]`

Thread pools, named after what they do.

| Key        | Default | Meaning                                                                                                                                                                                                                                                                                                                           |
| ---------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `search`   | `0`     | `serve` only: route search threads (`search-{i}`), one search each. `0` takes a quarter of the cores, at least one.                                                                                                                                                                                                               |
| `pipeline` | `0`     | Pipeline threads (`pipe-p{i}`), one per partition. Each reads its own gRPC streams, applies their writes and decodes its pools. `0` picks the largest divisor of `grpc.streams` up to half the cores; otherwise 1 to `grpc.streams`. Shard `i` feeds partition `i % pipeline`, so a divisor gives every partition as many shards. |

Besides these, `watch` runs a fixed two-thread runtime (`app`) for startup, the stats line and, in `blocks_meta` mode, the slot feed. `watch` logs the resolved counts at start (`threads`).

## `[server]`

Used by `serve` only (see [architecture.md](architecture.md#http-api)). Both addresses are bound before the engine starts, so a taken port fails at once.

| Key                   | Default            | Meaning                                                                                                                                                       |
| --------------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `api_addr`            | `"127.0.0.1:8080"` | Address of `POST /quote`, `/swap-instructions` and `/swap`.                                                                                                   |
| `ops_addr`            | `"127.0.0.1:9100"` | Address of `/health` and `/ready`. `0.0.0.0:9100` exposes them beyond the host.                                                                               |
| `drain_delay_ms`      | `0`                | After ctrl-c or `SIGTERM`, how long `/ready` answers 503 while the API keeps serving, so a load balancer moves traffic first. Skipped when the engine failed. |
| `read_timeout_ms`     | `5000`             | Request headers, and on the API the body, must arrive within this; a client that stops sending is cut off.                                                    |
| `shutdown_timeout_ms` | `5000`             | After the drain, one deadline for requests in flight and running searches; connections still open then are aborted. Keep it above `quote.timeout_ms`.         |

### `[server.ready]`

| Key                  | Default | Meaning                                                                                                                                      |
| -------------------- | ------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `startup_percent`    | `90`    | `/ready` first turns 200 once this share of the eligible pools is ready. Eligible: ready, or not ready for a reason that clears by itself.   |
| `floor_percent`      | `50`    | Once ready, `/ready` fails again only below this share. One pool or one stream shard dropping out leaves the service ready.                  |
| `max_clock_stall_ms` | `10000` | `/ready` fails, and the API answers `STALE_DATA`, when the Clock sysvar's slot has not moved for longer than this: the streams have stalled. |

### `[server.quote]`

| Key                | Default  | Meaning                                                                                                                                                  |
| ------------------ | -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `default_max_hops` | `3`      | Pools a route may pass when the request gives no `maxHops`.                                                                                              |
| `max_hops`         | `4`      | Largest `maxHops` a request may ask for.                                                                                                                 |
| `max_operations`   | `16`     | Maximum operations in a routed execution flow. Must be between 1 and 16.                                                                                 |
| `max_quotes`       | `100000` | Quotes one search may compute, all its widening attempts and split chunks together; a quote the session's memo answers is not counted. A work budget, not a deadline: filtering, pinning and ranking are not counted.           |
| `per_pair`         | `2`      | Pools kept per pair while searching (see [architecture.md](architecture.md#algorithm)); `0` keeps every one.                                             |
| `max_arrays`       | `8`      | Tick or bin arrays one quote may cross. The transaction that carries a route has to pass the same arrays ([dexes.md](dexes.md)).                         |
| `unique_dex_ids`   | `[]`     | DEX program IDs which are unique by default in cyclic-arbitrage requests; a non-empty request `uniqueDexIds` replaces this list.                         |
| `timeout_ms`       | `2000`   | How long a request waits for its search, queue time included, before it answers `TIMEOUT`. A search already running still finishes and keeps its thread. |
| `max_queued`       | `32`     | Searches that may wait for a thread. Past `[threads] search` running plus this many waiting, a request answers `OVERLOADED` at once.                     |

### `[server.swap]`

| Key                    | Default | Meaning                                                                                                                         |
| ---------------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `default_slippage_bps` | `50`    | Slippage of `otherAmountThreshold` when a request gives no `slippagePercent`; stored internally in basis points, at most 10000. |
| `max_quote_age_slots`  | `32`    | A `quoteResponse` whose `contextSlot` is further behind the market's Clock answers `QUOTE_EXPIRED`.                             |
| `blockhash_refresh_ms` | `2000`  | How often `serve` fetches the latest blockhash (`getLatestBlockhash`, `confirmed`) for `/swap`.                                 |
| `max_blockhash_age_ms` | `20000` | `/swap` answers `NO_BLOCKHASH` when the last fetched blockhash is older than this, for example while the RPC is down.           |

## Top level

| Key                   | Default | Meaning                                |
| --------------------- | ------- | -------------------------------------- |
| `stats_interval_secs` | `10`    | How often `watch` logs update counters |
