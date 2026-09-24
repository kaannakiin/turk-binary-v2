# Architecture

## Crates

| Crate              | Job                                                                      | Talks to the network?      |
| ------------------ | ------------------------------------------------------------------------ | -------------------------- |
| `apps/turk-binary` | CLI, config loading, logging                                             | No (uses the crates below) |
| `domain`           | Shared types: `DexKind`, `Slot`, `AccountUpdate`, `AccountFilter`        | No                         |
| `dex`              | What each DEX looks like on chain: program ID, pool filter, mint offsets | No                         |
| `rpc`              | Every JSON-RPC call                                                      | Yes, JSON-RPC only         |
| `grpc`             | Every Yellowstone gRPC stream                                            | Yes, gRPC only             |
| `market`           | Picks the pools to watch, keeps their latest state                       | Through `rpc` and `grpc`   |

Dependencies point one way:

```text
turk-binary ──▶ market ──▶ rpc ──┐
                   │  └──▶ grpc ─┤
                   └────▶ dex ───┴──▶ domain
```

## One gate per protocol

Only `rpc` may depend on `solana-rpc-client`, and only `grpc` may depend on `yellowstone-grpc-*`. `cargo deny` fails CI if any other crate tries. That gives one place for retries, rate limits and error handling.

## Data flow for `watch`

```text
config.toml ─▶ resolve universe ─▶ subscribe (gRPC) ─▶ snapshot (RPC) ─▶ live updates
                 │                    │                  │                  │
                 │                    │                  └─▶ account store ◀┘
                 └─ mints + pools + allowed/blocked DEXes
```

1. **Resolve.** Turn `mints` and `pools` from config into a list of pool addresses (see [configuration.md](configuration.md)).
2. **Subscribe.** Ask the gRPC stream for those addresses. Updates start queueing.
3. **Snapshot.** Fetch the same addresses once over RPC.
4. **Stream.** Apply queued and new updates.

Steps 2 and 3 run in that order on purpose. Subscribing first means nothing that changes during the snapshot is missed. The store keeps only the newest copy of each account (highest slot, then highest write version), so the overlap is harmless.

## gRPC hub

The hub runs `streams` Yellowstone streams (default 12). Each one is a shard.

- **Pools** are split across shards by address. The mapping is stable, so a pool always lands on the same shard.
- **Filter subscriptions** (by owner program) go to the one shard their name hashes to.
- **Empty shards** never connect.
- **Every send is the full filter set**, because Yellowstone replaces filters on each request. Lists longer than `max_pubkeys_per_filter` are split into several filters.

Each shard watches its own health:

- **Short drops.** The Yellowstone client reconnects by itself. With `recover_missed_data = true` it replays the missed slots and removes the duplicates.
- **Silent or lagging stream.** If nothing arrives for `recv_timeout_ms`, or messages are older than `max_message_delay_ms`, the shard rebuilds its client.
- **After a rebuild.** The shard emits `Reconnected` with its pool addresses, and `market` fetches just those again over RPC, because that gap cannot be replayed.

If one shard gives up (`reconnect.max_attempts`), the whole hub stops and `watch` exits with the error.

## Fork tracking

Updates stream at `processed` so they arrive as early as possible. A `processed` block can still lose to a sibling fork, and Yellowstone sends no "abandoned" message for it. So the store keeps two layers per account:

- **committed**: the newest state from a confirmed slot.
- **pending**: versions from slots that are not confirmed yet.

Each shard also follows its own slot stream, which carries every slot's parent and its `confirmed` / `dead` status.

- **When slot C is confirmed**: walk C's parents back to the previous confirmed slot. Pending versions on that chain are promoted to committed. Pending versions from other slots at or below C were on a losing fork and are dropped (`rolled_back`).
- **When a slot is dead**: its pending versions are dropped (`dead_dropped`).
- **Readers**: `head` returns the newest pending version, or the committed one if there is none. `committed` returns only confirmed state.
- **RPC snapshots** are read at `confirmed`, so they go straight into the committed layer.

Tracking is per shard because `write_version` only has meaning within one connection. Versions from the same slot are ordered by `write_version` only when they came over the same connection generation; a rebuilt connection may be talking to another node.

If a parent link is missing, slots below the hole are promoted as if canonical and `fork_gaps` is counted. Holes from before the subscription started are not counted.

**Alpenglow.** The design already follows the official recipe (buffer unconfirmed data, promote the confirmed bank, drop the rest). Once the Yellowstone proto ships `bank_id`, pending versions and the slot tree key on `(slot, bank_id)` instead of `slot`, because one slot may then carry more than one bank.
