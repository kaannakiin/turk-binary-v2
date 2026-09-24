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
2. **From `mints`**: on each enabled DEX, find pools where both sides are in `mints`. Three mints `A, B, C` give you every `A/B`, `A/C` and `B/C` pool. This uses `getProgramAccounts`, which many RPC providers block (HTTP 403). If yours does, `watch` stops and names the DEX. Use a provider that allows it, or list the pools under `pools`, which only needs `getMultipleAccounts`.
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
| `commitment`          | `"processed"` | See [Commitment values](#commitment-values)                                               |
| `timeout_ms`          | `10000`       | Per-request timeout                                                                       |
| `max_in_flight`       | `8`           | Max concurrent RPC requests                                                               |
| `retry.max_attempts`  | `5`           | Attempts for timeouts, 429s, 5xx and "node behind" errors. Other errors fail immediately. |
| `retry.base_delay_ms` | `100`         | First backoff; doubles each attempt                                                       |
| `retry.max_delay_ms`  | `5000`        | Backoff cap                                                                               |

## `[grpc]`

### Streams

| Key                      | Default       | Meaning                                                                                                     |
| ------------------------ | ------------- | ----------------------------------------------------------------------------------------------------------- |
| `commitment`             | `"processed"` | See [Commitment values](#commitment-values)                                                                 |
| `streams`                | `12`          | Parallel gRPC streams (shards). Pools are split across them by address. Shards with no pools never connect. |
| `max_pubkeys_per_filter` | `100`         | Longer address lists are split into several filters, because providers cap accounts per filter              |
| `compression`            | `"gzip"`      | `none`, `gzip` or `zstd`. Use `none` when the gRPC node is on the same machine.                             |
| `recv_timeout_ms`        | `10000`       | Reconnect a shard that has received nothing for this long. `0` turns it off.                                |
| `max_message_delay_ms`   | `10000`       | Reconnect a shard whose messages are older than this (server `created_at`). `0` turns it off.               |
| `connect_timeout_ms`     | `10000`       | Connect timeout                                                                                             |
| `max_message_bytes`      | `67108864`    | Largest accepted message                                                                                    |
| `event_buffer`           | `16384`       | Updates queued between the streams and the store                                                            |
| `command_buffer`         | `128`         | Queued subscribe/unsubscribe requests per shard                                                             |

Every shard also subscribes to slot updates. The receive timer resets on every message, slot updates included, so a pool with no trades never trips it. It only fires when the whole stream goes quiet.

### Reconnects

| Key                         | Default | Meaning                                                                     |
| --------------------------- | ------- | --------------------------------------------------------------------------- |
| `recover_missed_data`       | `true`  | On a short drop, replay the missed slots (Yellowstone `from_slot` + dedup)  |
| `slot_retention`            | `250`   | Slots of history the replay dedup keeps                                     |
| `stream_reconnect_attempts` | `5`     | Quick reconnects inside the Yellowstone client before the shard rebuilds it |
| `stream_reconnect_base_ms`  | `100`   | First quick-reconnect delay                                                 |
| `reconnect.max_attempts`    | `10`    | Full rebuilds in a row before `watch` exits with an error                   |
| `reconnect.base_delay_ms`   | `500`   | First full-rebuild delay                                                    |
| `reconnect.max_delay_ms`    | `30000` | Full-rebuild delay cap                                                      |

After a full rebuild, that shard's pools are fetched again over RPC, because the gap cannot be replayed.

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

### Where the defaults come from

The defaults follow two production Solana routers that stream from Yellowstone: OKX Pallas ([configuration](https://github.com/okx/dex-solana-binary/blob/master/docs/configuration.md#grpc-streaming-tuning)) and Metis ([self-host](https://metis.builders/docs/self-host#yellowstone-grpc-streaming)). Both use `processed`, 12 streams, gzip, a 10 s message delay limit and a 16384-update buffer. The transport values come from Metis. On `recv_timeout_ms` they differ: Metis uses 3 s, OKX 60 s. We use 10 s. With slot updates arriving several times a second, 10 s of silence is about 25 missed slots, well past gaps from skipped leaders. It still catches a stuck stream before HTTP/2 keepalive would (about 20 s).

## Top level

| Key                   | Default | Meaning                                |
| --------------------- | ------- | -------------------------------------- |
| `stats_interval_secs` | `10`    | How often `watch` logs update counters |
