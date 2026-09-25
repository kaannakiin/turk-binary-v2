# Architecture

## Crates

| Crate              | Job                                                                                         | Talks to the network?      |
| ------------------ | ------------------------------------------------------------------------------------------- | -------------------------- |
| `apps/turk-binary` | CLI, config loading, logging                                                                | No (uses the crates below) |
| `domain`           | Shared types: `DexKind`, `Slot`, `AccountUpdate`, `AccountFilter`, `ChainClock`             | No                         |
| `dex`              | What each DEX looks like on chain: program ID, pool filter, and every account a quote needs | No                         |
| `rpc`              | Every JSON-RPC call, rate-limited                                                           | Yes, JSON-RPC only         |
| `grpc`             | Every Yellowstone gRPC stream: subscriptions, reconnects, replay, provider probes           | Yes, gRPC only             |
| `market`           | Picks the pools, keeps every account they depend on subscribed and in sync, answers reads   | Through `rpc` and `grpc`   |

Dependencies point one way:

```text
turk-binary ──▶ market ──▶ rpc ──┐
                   │  └──▶ grpc ─┤
                   └────▶ dex ───┴──▶ domain
```

## One gate per protocol

Only `rpc` may depend on `solana-rpc-client`, and only `grpc` may depend on `yellowstone-grpc-*`. `cargo deny` fails CI if any other crate tries. That gives one place for retries, rate limits and error handling.

## Dependency closures

A pool is not enough to quote a swap. The quote also reads vaults, fee configs, tick or bin arrays, bitmap extensions, oracles, mints (for Token-2022 transfer fees) and the Clock sysvar. `dex::closure` derives that full set, the pool's **closure**, from the pool's bytes. It is pure: no I/O.

- **Fixed dependencies** sit at known offsets in the pool (vaults, configs, mints) or at fixed-seed PDAs (Pump's `Global`, `FeeConfig`).
- **Tick and bin arrays** come from the pool's bitmap (CLMM, DLMM) plus its bitmap extension. Every initialized array is included, not a window around the price. Whirlpools have no bitmap, so every possible tick-array PDA is included as optional; subscribing to the ones that do not exist yet is what makes their creation visible.
- **Multi-step closures** report the accounts they still need in `awaiting`. A CLMM or DLMM closure waits for its bitmap extension; DAMM v1 waits for its vaults, which name their LP mints and token accounts. The engine derives again once those accounts are known.
- Every dependency carries a **role**, a **scope** (`Pool`, or `Shared` by many pools), a **presence** (`Required`, or `Optional` when it may legitimately not exist) and whether the swap math reads it or only the swap instruction needs it.
- `dex::structural_ranges` lists the bytes of an account whose change can change the closure (a bitmap, a vault address). Updates outside them never trigger a re-derivation.

[dexes.md](dexes.md#dependencies) lists each DEX's closure.

## Data flow for `watch`

```text
config ─▶ resolve universe ─▶ derive closures ─▶ subscribe groups ─▶ Effective ─▶ seed (RPC) ─▶ live
            (pools + bytes)     (dex, pure)        (grpc)              (slot)      at barrier
```

1. **Resolve.** Turn `mints` and `pools` into pools, keeping each pool's bytes (see [configuration.md](configuration.md)).
2. **Derive.** Compute every pool's closure.
3. **Subscribe.** Each pool's pool-scoped accounts form one group; each shared account is its own group. Groups go to the hub.
4. **Effective.** The hub reports the slot from which the server applies the new filters.
5. **Seed.** Every newly subscribed account is read over RPC at `confirmed`, no earlier than that slot plus `settle_slots` (the **barrier**). A read from before the barrier could miss a write the stream will never resend, because accounts nobody writes are never streamed. A streamed update after the filter is effective carries the full account and settles it without a read.
6. **Live.** From then on the stream keeps the account current.

When a structural account changes (a bitmap bit flips, an extension is created), the closure is derived again and only the difference is subscribed and seeded. Nothing is rebuilt from scratch.

## gRPC hub

- **Groups and sharding.** A pool's group lands on one of `streams` shards by rendezvous hashing of the pool address, so a pool and all its arrays share a stream, a connection generation and a slot tree. Shared accounts go to one separate `Shared` stream. Streams without groups never connect.
- **Every request is the full filter set**, because Yellowstone replaces filters on each send. Pubkeys are split into filters of at most `max_pubkeys_per_filter`. Changes are coalesced for `filter_flush_ms` and sent on the open stream; no reconnect.
- **Confirming a filter change.** The plugin sends no acknowledgement, so filter names carry a sequence number and the server tags every update with the names it matched. The first update tagged with a new sequence proves the switch; its slot is the effective slot. A request never carries `ping`: the plugin treats a request with `ping` as a keepalive and leaves its filters unchanged. Server pings are answered with a ping-only request.
- **Clock on every stream.** `SysvarC1ock11111111111111111111111111111111` updates every slot, so every stream subscribes it. It is the heartbeat, the evidence of the stream's current slot, the replay checkpoint, and (on the shared stream only) the chain clock readers use.
- **Limits.** A group that would push a request over a provider limit is refused alone (`Rejected`), not the whole stream. Limit errors from the server lower the limits and the stream reconnects.

### Reconnects and replay

The Yellowstone client's own reconnect is off: it injects filters of its own and, when replay is out of range, resumes from the head without saying so. Each stream owns its reconnects instead:

1. On a drop the stream emits `Down`, and its pools stop being ready.
2. It asks the server for its oldest replayable slot. If the last slot seen minus `replay_margin_slots` is still in range, it reconnects with `from_slot`.
3. If the replay starts more than `replay_skip_tolerance` slots late, or replay is off, unsupported or out of range, the stream emits `Gap` with all its keys once the new connection is effective. Only those keys are re-seeded, at the new barrier. Nothing else is touched.
4. If the replay reaches the pre-drop tip, the stream emits `Resumed` and nothing needs a read.

Account updates older than `max_message_delay_ms` make a stream reconnect, except while a replayed connection catches up: replayed account updates keep the time the plugin first saw them, and the replay also carries everything written while the stream was down. Lag counts again from the first current account update. Slot statuses and pings are not used, because the plugin stamps replayed ones with the time it sends them.

`reconnect.max_attempts = 0` retries forever. Only fatal errors (bad credentials, a request the server can never accept) stop the hub.


## Engine

`market::Engine` owns all state in one task and never waits on RPC. Seeds, repairs and audits run in spawned tasks and report back, so a slow endpoint delays seeds but never the stream.

- **Repair queue.** Keys needing a read are batched into tickets of up to 100 keys, pool and structural accounts first, speculative ones (Whirlpool tick arrays that may not exist) last. At most `repair_concurrency` tickets are in flight, and a key is never read twice at once.
- **Epochs.** A gap bumps each affected key's epoch; a read dispatched before the gap cannot mark the key live.
- **Drift audit.** Every `audit_interval_ms`, up to 100 live keys are read and compared with what the store held at that slot. A difference replaces the stored copy and is counted.

## Readiness

A pool is **ready** when every dependency is live, every required one exists and has an accepted owner, its streams are up and its closure is complete and verified. Otherwise it reports why, most structural cause first: `Closed`, `Unsubscribable`, `Unverified`, `StreamDown`, `Awaiting`, `Syncing`, `Missing`, `OwnerMismatch`. A missing array disables one pool, not the bot.

An address the System program owns with no data counts as absent even when it holds lamports: anyone can send lamports to a PDA, and the programs treat such an account as uninitialized (seen live on two Whirlpool oracle PDAs).

`market::MarketReader` is the read side for the quote layer:

- `pool_view(pool, layer)`: every dependency read under one store lock, with the pool's readiness and the chain clock.
- `clock(layer)`: the Clock sysvar, never the host clock.
- `subscribe()`: a `PoolChanged` for each change to a pool's dependencies.

## Fork tracking

Updates stream at `processed` so they arrive as early as possible. A `processed` block can still lose to a sibling fork, and Yellowstone sends no "abandoned" message for it. So the store keeps two layers per account:

- **committed**: the newest state from a confirmed slot.
- **pending**: versions from slots that are not confirmed yet.

Slot statuses come from `slot_source`:

- **`slots`**: every stream carries its own slot updates and keeps its own slot tree, because `write_version` only has meaning within one connection.
- **`blocks_meta`**: for providers that refuse `slots`. One extra connection streams confirmed block metadata into one global tree. Dead slots are not reported there, so losing forks are rolled back when the next slot is confirmed rather than when they die.
- **`auto`**: a probe at start picks `slots` if the provider accepts it.

Resolution:

- **When slot C is confirmed**: walk C's parents back to the previous confirmed slot. Pending versions on that chain are promoted to committed. Pending versions from other slots at or below C were on a losing fork and are dropped (`rolled_back`). Only accounts with pending versions at or below C are visited.
- **When a slot is dead**: its pending versions are dropped (`dead_dropped`).
- **Readers**: `Head` returns the newest pending version, or the committed one. `Committed` returns only confirmed state.
- **RPC reads** are taken at `confirmed`, so they go straight into the committed layer. An account that does not exist is stored as absent at that slot, which is state too.

If a parent link is missing, slots below the hole are promoted as if canonical, `fork_gaps` is counted, and the promoted accounts are re-read. The chain starts at the first `processed` slot a stream sees, and again after every drop; the older statuses resent on connect fill in parents but do not move that start, so the hole below it is not a gap. A drop without replay leaves a hole that the stream's `Gap` re-reads anyway.

**Alpenglow.** The design already follows the official recipe (buffer unconfirmed data, promote the confirmed bank, drop the rest). Once the Yellowstone proto ships `bank_id`, pending versions and the slot tree key on `(slot, bank_id)` instead of `slot`, because one slot may then carry more than one bank.

## Provider probes

`turk-binary probe` checks what the gRPC provider supports: the `slots` filter, Clock streaming, filter-name tagging, ping-only requests, replay (window, a recent `from_slot`, the out-of-range error), filter limits and which older slot statuses a new subscription is sent (`slot-backlog`). All checks are read-only. Run it once per provider and set `slot_source` and the limits from its output.

Measured 2026-09-24 on the configured provider: `slots` accepted, Clock streamed every slot, filter names tagged on updates, ping-only requests keep the filters, a replay window of about 3,000 slots, the out-of-range error as the plugin source defines it, and at least 20,000 pubkeys per filter and 200 filters per request. Measured 2026-09-25: a new subscription is also sent `finalized` statuses, with parents, for about 22 older slots (up to 31 back), mostly after its first `processed` one.

## RPC budget

Designed for a free-tier key (about 10 requests per second, `getProgramAccounts` possibly blocked):

- `max_rps` paces every request.
- Closures never use `getProgramAccounts`; only pool discovery by mint does (use `pools` if the provider blocks it).
- Seeds are one `getMultipleAccounts` per 100 keys. A Whirlpool with `tick_spacing = 1` has about 10,000 possible tick arrays, about 100 reads.
- After a gap only that stream's keys are read again.
- In steady state only the audit reads: one request per `audit_interval_ms`.
