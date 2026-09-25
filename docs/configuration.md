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

| Key                   | Default       | Meaning                                                                                   |
| --------------------- | ------------- | ----------------------------------------------------------------------------------------- |
| `commitment`          | `"processed"` | Commitment for universe resolution. Seeds and repairs always read at `confirmed`.         |
| `timeout_ms`          | `10000`       | Per-request timeout                                                                       |
| `max_in_flight`       | `8`           | Max concurrent RPC requests                                                               |
| `max_rps`             | `8`           | Requests per second, paced evenly across every method. `0` turns pacing off.              |
| `retry.max_attempts`  | `5`           | Attempts for timeouts, 429s, 5xx and "node behind" errors. Other errors fail immediately. |
| `retry.base_delay_ms` | `100`         | First backoff; doubles each attempt                                                       |
| `retry.max_delay_ms`  | `5000`        | Backoff cap                                                                               |

## `[grpc]`

### Streams

| Key                      | Default       | Meaning                                                                                                               |
| ------------------------ | ------------- | --------------------------------------------------------------------------------------------------------------------- |
| `commitment`             | `"processed"` | See [Commitment values](#commitment-values)                                                                           |
| `streams`                | `12`          | Pool shards. A pool and its pool-only dependencies share one; shared accounts use one extra stream per partition. Empty shards never connect. |
| `slot_source`            | `"auto"`      | `slots`, `blocks_meta` or `auto`. See [architecture.md](architecture.md#fork-tracking).                               |
| `max_pubkeys_per_filter` | `100`         | Longer address lists are split into several filters. Lowered automatically if the server reports a limit.             |
| `max_account_filters`    | unset         | Max account filters per request. Unset means learn it from the server's error.                                        |
| `max_txn_pubkeys_per_filter` | `100`     | The same split for the transaction-status filters on shards. Lowered automatically if the server reports a limit.     |
| `max_txn_filters`        | unset         | Max transaction-status filters per request. Unset means learn it from the server's error.                            |
| `warn_request_bytes`     | `2097152`     | Log a warning when a subscribe request is larger                                                                      |
| `max_request_bytes`      | `4000000`     | Refuse groups that would make a request larger                                                                        |
| `compression`            | `"gzip"`      | `none`, `gzip` or `zstd`. Use `none` when the gRPC node is on the same machine.                                       |
| `recv_timeout_ms`        | `10000`       | Reconnect a stream that has received nothing for this long. `0` turns it off.                                         |
| `max_message_delay_ms`   | `10000`       | Reconnect a stream whose messages are older than this (server `created_at`). `0` turns it off.                        |
| `connect_timeout_ms`     | `10000`       | Connect timeout                                                                                                       |
| `max_message_bytes`      | `67108864`    | Largest accepted message                                                                                              |
| `event_buffer`           | `16384`       | Events queued between the streams and each partition's engine                                                         |
| `filter_flush_ms`        | `200`         | Subscription changes are collected this long and sent as one filter update                                            |
| `filter_ack_timeout_ms`  | `2000`        | If no update confirms a filter change in this time, it is assumed effective at the latest slot                        |

Every stream also subscribes to the Clock sysvar, which updates every slot. The receive timer resets on every message, so a pool with no trades never trips it.

### Reconnects

| Key                       | Default | Meaning                                                               |
| ------------------------- | ------- | --------------------------------------------------------------------- |
| `replay`                  | `true`  | Reconnect with `from_slot` to replay what was missed                  |
| `replay_margin_slots`     | `4`     | Replay starts this many slots before the last slot seen               |
| `replay_skip_tolerance`   | `8`     | A replay that starts more than this many slots late counts as a gap   |
| `reconnect.max_attempts`  | `0`     | Failed reconnects in a row before `watch` exits. `0` retries forever. |
| `reconnect.base_delay_ms` | `500`   | First reconnect delay                                                 |
| `reconnect.max_delay_ms`  | `30000` | Reconnect delay cap                                                   |

When a reconnect cannot replay, only that stream's accounts are read again over RPC (see [architecture.md](architecture.md#reconnects-and-replay)).

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

| Key                          | Default | Meaning                                                                                              |
| ---------------------------- | ------- | ---------------------------------------------------------------------------------------------------- |
| `settle_slots`               | `4`     | A seed read must be at least this many slots after the slot its filter became effective              |
| `repair_concurrency`         | `2`     | Seed and repair reads in flight at once                                                              |
| `repair_batch`               | `100`   | Keys per read (`getMultipleAccounts` takes at most 100)                                              |
| `repair_retry.base_delay_ms` | `500`   | First delay before a failed read is retried                                                          |
| `repair_retry.max_delay_ms`  | `30000` | Retry delay cap. Failed reads are retried until they succeed.                                        |
| `audit_interval_ms`          | `10000` | One drift check of up to 100 accounts per interval. `0` turns it off.                                |
| `stream_swap_accounts`       | `true`  | Also subscribe accounts only the swap instruction needs (vaults the math does not read, DLMM oracle) |
| `tick_ms`                    | `50`    | How often closures, reads and readiness are brought up to date                                       |
| `txn_wait_ms`                | `400`   | How long a shard's transaction writes wait for the transaction status before they are applied anyway |
| `pipeline_threads`           | `2`     | Writer threads (partitions). Shard `i` feeds partition `i % pipeline_threads`; must be 1 to `grpc.streams` |

## `[route]`

| Key             | Default | Meaning                                                                                                             |
| --------------- | ------- | ------------------------------------------------------------------------------------------------------------------- |
| `route_threads` | `4`     | Decode and quote threads, separate from the writers. Each owns the pools its hash picks; must be at least 1 |
| `probe_amount`  | `1000000` | `watch` quotes this raw input amount both ways through every decoded pool each stats tick and logs the outcomes per DEX (`quote probe`); `0` turns it off |

## Top level

| Key                   | Default | Meaning                                |
| --------------------- | ------- | -------------------------------------- |
| `stats_interval_secs` | `10`    | How often `watch` logs update counters |
