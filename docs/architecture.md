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
| `quoter`           | Decodes a pool's accounts and computes swap quotes (DEX math and SDK binds); pure           | No                         |
| `route`            | Route threads: decode each pool as the market changes it, publish state for quotes         | No (reads `market`)        |

Dependencies point one way:

```text
turk-binary ──▶ route ──▶ quoter ──▶ dex ──▶ domain
     │            └─────▶ market
     └──────────▶ market ──▶ rpc ──┐
                     │  └──▶ grpc ─┤
                     └────▶ dex ───┴──▶ domain
```

`market` never depends on `quoter`: DEX SDKs, with their own Anchor and Solana versions, compile only where quotes are computed.

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

- **Groups and sharding.** A pool's group lands on one of `streams` shards by rendezvous hashing of the pool address. The group holds the pool's pool-scoped accounts (vaults, arrays, oracle), so they share a stream, a connection generation and a slot tree. Shared accounts (Clock, mints, configs, DAMM v1 vault state, any account two pools use) go to the partition's own `Shared` stream (see [Partitions](#partitions)), so a pool's accounts can arrive over two streams with no order between them. Streams without groups never connect.
- **Transaction statuses on shards.** Each shard also subscribes `transactions_status` for its group accounts (Clock excluded; vote and failed transactions excluded). The plugin sends a transaction's status after every account write of that transaction on the same stream, so the status closes the group of writes that carry its signature. Measured 2026-09-25 with `just txn-probe` (30 min, 280 pools, Yellowstone 15.2.1 on Agave 4.2.2): of 44,434 statuses none arrived before one of its writes or after its slot's `processed` status, and no group went 2 s without one. A 45-minute rerun on an otherwise idle machine put the status 15–133 µs after the group's last write at p50 and under 0.6 ms at p99 for every DEX (max 48 ms). The first run, taken while the same machine was compiling, showed a p99 of 107–169 ms for Whirlpool and DLMM, most likely the probe itself waiting for CPU. In the rerun, a new transaction never wrote a pool before the previous transaction's status arrived, so releasing a group early on the next write would gain nothing.
- **Every request is the full filter set**, because Yellowstone replaces filters on each send. Pubkeys are split into filters of at most `max_pubkeys_per_filter`. Changes are coalesced for `filter_flush_ms` and sent on the open stream; no reconnect.
- **Confirming a filter change.** The plugin sends no acknowledgement, so filter names carry a sequence number and the server tags every update with the names it matched. The first update tagged with a new sequence proves the switch; its slot is the effective slot. A request never carries `ping`: the plugin treats a request with `ping` as a keepalive and leaves its filters unchanged. Server pings are answered with a ping-only request.
- **Clock on every stream.** `SysvarC1ock11111111111111111111111111111111` updates every slot, so every stream subscribes it. It is the heartbeat, the evidence of the stream's current slot, the replay checkpoint, and (on the shared stream only) the chain clock readers use.
- **Limits.** Nothing assumes a provider's limits; they are learned from its errors. A group that would push a request over a limit is refused alone (`Rejected`), not the whole stream. Limit errors from the server lower the limits and the stream reconnects. The plugin words account and transaction-status filter limits the same way, so a reported limit is charged to whichever part of the request exceeds it. A key the server does not allow in a filter (`account_include_reject`) is first left out of the transaction-status filter; if the account filter refuses it too, only the groups holding it are rejected.

### Reconnects and replay

The Yellowstone client's own reconnect is off: it injects filters of its own and, when replay is out of range, resumes from the head without saying so. Each stream owns its reconnects instead:

1. On a drop the stream emits `Down`, and its pools stop being ready.
2. It asks the server for its oldest replayable slot. If the last slot seen minus `replay_margin_slots` is still in range, it reconnects with `from_slot`.
3. If the replay starts more than `replay_skip_tolerance` slots late, or replay is off, unsupported or out of range, the stream emits `Gap` with all its keys once the new connection is effective. Only those keys are re-seeded, at the new barrier. Nothing else is touched.
4. If the replay reaches the pre-drop tip, the stream emits `Resumed` and nothing needs a read.

Account updates older than `max_message_delay_ms` make a stream reconnect, except while a replayed connection catches up: replayed account updates keep the time the plugin first saw them, and the replay also carries everything written while the stream was down. Lag counts again from the first current account update. Slot statuses and pings are not used, because the plugin stamps replayed ones with the time it sends them.

`reconnect.max_attempts = 0` retries forever. Only fatal errors (bad credentials, a request the server can never accept) stop the hub.

To exercise this on a live stream, run `just watch-release` and, in another terminal, `sudo scripts/net_fault.sh drop 30` (packets vanish, the stream has to notice) or `sudo scripts/net_fault.sh reset 5` (connections are reset at once). The script cuts only that process's TCP connections, with pf, and restores them after the given seconds. Expect `downs` and `resumed` to rise, `gaps` and `drift` to stay at 0, and every pool to be ready again. With `replay = false` the same cut exercises the `Gap` path instead.

## Partitions

`sync.pipeline_threads` (default 2) splits the writers. Shard `i` feeds partition `i % pipeline_threads`, so a pool's partition follows from its shard. Each partition is one `Engine` on its own OS thread (`market-p{i}`, with a single-threaded tokio runtime) and owns its pools' store, closures, sync state, repairs, audit and slot trees.

Partitions share nothing but the read side:

- Each partition has its own shared stream, so Clock, mints and configs are subscribed once per partition. A shared account's `Effective`, seeds and fork state therefore never cross a partition.
- The slot feed (`blocks_meta` mode) sends its slot statuses to every partition.
- All partitions publish into one lock-free snapshot table and one `PoolChanged` feed, and their stats are summed (slots: the newest).
- RPC load stays bounded by the `rpc` gateway's global `max_in_flight` and `max_rps`. `repair_concurrency` applies per partition.

With `pipeline_threads = 1` everything runs on one writer thread, as before.

## Engine

`market::Engine` owns one partition's state in one task and never waits on RPC. Seeds, repairs and audits run in spawned tasks and report back, so a slow endpoint delays seeds but never the stream.

- **Transaction groups.** A swap writes the pool and both vaults in separate messages. Applied one by one, a reader could see one vault after the swap and the other before it. The txn probe measured this at 66–86% of pool updates. So a shard write that carries a transaction signature is held until that transaction's status arrives, then the whole group is applied at once and each affected pool gets one `PoolChanged`. Writes without a signature (sysvars) and shared-stream writes are applied at once. A group whose status never comes is applied after `txn_wait_ms`, or before its slot is confirmed, whichever is first, and counted as `txn_orphans`. A dropped stream discards its open groups; replay or the `Gap` re-read brings them back. Pools whose state spans both streams (DAMM v1, accounts shared by two pools) get no such guarantee.
- **Repair queue.** Keys needing a read are batched into tickets of up to 100 keys, pool and structural accounts first, speculative ones (Whirlpool tick arrays that may not exist) last. At most `repair_concurrency` tickets are in flight, and a key is never read twice at once.
- **Epochs.** A gap bumps each affected key's epoch; a read dispatched before the gap cannot mark the key live.
- **Drift audit.** Every `audit_interval_ms`, up to 100 live keys are read and compared with what the store held at that slot. A key is compared only once its own stream has confirmed that slot, so every write up to it has arrived. Owner, lamports and data are compared as they are, so a funded but uninitialized address has to match too. A difference replaces the stored copy and is counted.

## Readiness

A pool is **ready** when every dependency is live, every required one exists and has an accepted owner, its streams are up and its closure is complete and verified. Otherwise it reports why, most structural cause first: `Closed`, `Unsubscribable`, `Unverified`, `StreamDown`, `Awaiting`, `Syncing`, `Missing`, `OwnerMismatch`. A missing array disables one pool, not the bot.

An address the System program owns with no data counts as absent even when it holds lamports: anyone can send lamports to a PDA, and the programs treat such an account as uninitialized (seen live on two Whirlpool oracle PDAs).

`market::MarketReader` is the read side for the quote layer:

- `pool_view(pool)`: the pool as the engine last published it: every dependency except the Clock, its readiness, and `cross_stream` when an account a swap writes rides the shared stream (DAMM v1, accounts two pools share), so the view may hold part of a transaction. Reads take no lock: the engine publishes an immutable view per pool (`arc-swap`) after each applied transaction group, seed, audit fix or readiness change.
- `clock()`: the Clock sysvar, never the host clock. It has its own cell, so a Clock update rebuilds no pool view and sends no `PoolChanged`. Every partition writes its own stream's Clock into that cell, so only a newer slot replaces it; a partition behind another, or a rolled-back fork, never moves the clock back.
- `subscribe()`: a `PoolChanged` for each published change to a pool: once per transaction group, when a fork rollback or a dead slot moves an account back, and when the pool's readiness, dependencies or `cross_stream` change without any write (a stream drop, a closure change). A readiness event carries the partition's newest confirmed slot.

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
- **A dropped version that was the head** (rolled back or dead) moves the account back to the next pending version or the committed one. That is published like a write: the pools depending on it get a new view and a `PoolChanged`, and a structural or existence change re-derives the closure or readiness.
- **An update for a slot that is already confirmed**: this happens with replay after a reconnect, or when confirmations come over the separate slot-feed connection. The update is never put in pending, because the next confirmation would roll it back. If its slot is on the confirmed canonical chain (the last 512 slots are remembered), it goes straight to committed. Otherwise it is dropped and the account is read again (`late`).
- **Readers**: `Head` returns the newest pending version, or the committed one. `Committed` returns only confirmed state.
- **RPC reads** are taken at `confirmed`, so they go straight into the committed layer. An account that does not exist is stored as absent at that slot, which is state too.

If a parent link is missing, slots below the hole are promoted as if canonical, `fork_gaps` is counted, and the promoted accounts are re-read. The chain starts at the first `processed` slot a stream sees, and again after every drop; the older statuses resent on connect fill in parents but do not move that start, so the hole below it is not a gap. A drop without replay leaves a hole that the stream's `Gap` re-reads anyway.

**Alpenglow.** The design already follows the official recipe (buffer unconfirmed data, promote the confirmed bank, drop the rest). Once the Yellowstone proto ships `bank_id`, pending versions and the slot tree key on `(slot, bank_id)` instead of `slot`, because one slot may then carry more than one bank.

## Route threads

`route.route_threads` (default 4) sets the decode and quote threads, separate from the `pipeline_threads` writers. Each is one OS thread (`route-r{i}`) and owns the pools whose address hashes to it.

- **Input.** Every thread subscribes to `MarketReader::subscribe()`, drains whatever `PoolChanged` events are queued into one set, and handles each of its pools once per batch. A thread that falls behind the feed (`Lagged`) rescans all its pools instead.
- **Incremental decode.** Per pool the thread remembers, for every dependency, the update order, owner, lamports and data buffer it last decoded. Any difference counts as a change: a fork rollback moves the order back and RPC seeds share one write version, so "newer" would miss them. Only changed accounts go through `quoter::VenueState::apply`; a dependency that leaves the closure is applied as absent.
- **Readiness.** Only `Ready` pools are decoded. `Closed` and `Invalid` pools drop their state.
- **Output.** Each pool's state is published into a lock-free table together with the `PoolView` it was decoded from. `route::QuoteReader::quote` runs on the caller's thread and refuses when the market has published a newer view than the one decoded (`Stale`), the pool is not ready, a dependency failed to decode, or no Clock is known yet. Time-dependent inputs (fees by epoch, activation times) come from the Clock sysvar at quote time, so a Clock update needs no re-decode.
- **Panics.** A venue panic while decoding or quoting is caught: the pool's state is discarded and rebuilt on its next change, and `panics` counts it.
- **Probe.** `watch` exercises the quote path on live state: each stats tick it quotes `route.probe_amount` both ways through every decoded pool on a blocking thread (`QuoteReader::probe`) and logs, per DEX, how many quoted and why the rest refused (`quote probe`), with the sweep time and the slowest quote.

## LiteSVM oracle

`oracle/` is its own Cargo workspace, outside the bot's dependency graph and `cargo deny`: LiteSVM 0.16 and its Solana v4 stack cannot share a lockfile with `domain` (`solana-address` ~2.6 against ^2.8). It never links `quoter`.

1. `just snapshot` (`turk-binary snapshot`) syncs the market like `watch`, then writes up to `--per-dex` ready pools per DEX with the accounts their views hold and the Clock.
2. `just oracle` dumps the deployed bytecode of every program a swap touches (`scripts/dump_programs.py`, public endpoint; `oracle/programs/programs.tsv` records each program's deploy slot and hash), loads it into LiteSVM with mainnet's Rent sysvar and the snapshot's Clock, and runs each pool's swaps both ways at fractions of the input reserve and a fixed ladder of sizes. The instructions come from `oracle/arb-swap-ix`, the previous repo's builders. Accounts a swap passes but no quote reads come from the public endpoint. The user's token accounts are created by the ATA program, so Token-2022 accounts get their extensions. What the program paid, or why it refused, is written to `crates/quoter/src/tests/fixtures/svm/`.
3. `quoter`'s `svm` tests rebuild each pool's closure from the snapshot bytes with `dex::closure`, decode it as the route threads do and require every quote to equal the program's payout, and every refusal to be a refusal.

## Provider probes

`turk-binary probe` checks what the gRPC provider supports: the `slots` filter, Clock streaming, filter-name tagging, ping-only requests, replay (window, a recent `from_slot`, the out-of-range error), account and transaction-status filter limits and which older slot statuses a new subscription is sent (`slot-backlog`). All checks are read-only. Run it once per provider and set `slot_source` and the limits from its output.

`just txn-probe [minutes]` (`turk-binary txn-probe`) measures, per DEX, how one transaction's account writes and its `transactions_status` message arrive on the pool's stream. It picks up to `--per-dex` pools from the universe, subscribes to their pool-only keys plus a status filter on them, and their shared keys on a second stream. It reports:

- status messages that arrive before one of their writes (expected 0);
- writes without a signature;
- groups whose writes are interleaved with other groups;
- the share of pool updates that would publish a half-applied transaction if every write were published on its own;
- the delay from a group's last write to its status;
- groups with no status within `--orphan-after-ms`;
- statuses that arrive after their slot's `processed` status;
- how shared-stream writes of the same transaction line up with the pool stream's status.

Every message's metadata (no account data, no endpoint) goes to `target/txn-probe.tsv`.

Measured 2026-09-24 on the configured provider: `slots` accepted, Clock streamed every slot, filter names tagged on updates, ping-only requests keep the filters, a replay window of about 3,000 slots, the out-of-range error as the plugin source defines it, and at least 20,000 pubkeys per filter and 200 filters per request. Measured 2026-09-25: a new subscription is also sent `finalized` statuses, with parents, for about 22 older slots (up to 31 back), mostly after its first `processed` one.

## RPC budget

Designed for a free-tier key (about 10 requests per second, `getProgramAccounts` possibly blocked):

- `max_rps` paces every request.
- Closures never use `getProgramAccounts`; only pool discovery by mint does (use `pools` if the provider blocks it).
- Seeds are one `getMultipleAccounts` per 100 keys. A Whirlpool with `tick_spacing = 1` has about 10,000 possible tick arrays, about 100 reads.
- After a gap only that stream's keys are read again.
- In steady state only the audit reads: one request per `audit_interval_ms`.
