# Architecture

## Crates

| Crate              | Job                                                                                          | Talks to the network?      |
| ------------------ | -------------------------------------------------------------------------------------------- | -------------------------- |
| `apps/turk-binary` | CLI, config loading, logging                                                                 | No (uses the crates below) |
| `domain`           | Shared types: `DexKind`, `Slot`, `AccountUpdate`, `AccountFilter`, `ChainClock`              | No                         |
| `dex`              | What each DEX looks like on chain: program ID, pool filter, and every account a quote needs  | No                         |
| `rpc`              | Every JSON-RPC call, rate-limited                                                            | Yes, JSON-RPC only         |
| `grpc`             | Every Yellowstone gRPC stream: subscriptions, reconnects, provider probes                    | Yes, gRPC only             |
| `market`           | Picks the pools, keeps every account they depend on subscribed and in sync, answers reads    | Through `rpc` and `grpc`   |
| `quoter`           | Decodes a pool's accounts and computes swap quotes (DEX math and SDK binds); pure            | No                         |
| `graph`            | Token graph built once from the universe: mints, pools, edges, per-pool activity bits        | No                         |
| `route`            | Decodes each pool on the pipeline thread that publishes it; quotes against the decoded state | No (reads `market`)        |

Dependencies point one way:

```text
turk-binary ──▶ route ──▶ quoter ──▶ dex ──▶ domain
     │            ├─────▶ graph ──▶ market
     │            └─────▶ market
     ├──────────▶ graph
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

- **Groups and sharding.** A pool's group lands on one of `streams` shards by rendezvous hashing of the pool address. The group holds the pool's pool-scoped accounts (vaults, arrays, oracle), so they share a stream, a connection generation and a slot tree. Shared accounts (Clock, mints, configs, DAMM v1 vault state, any account two pools use) go to the partition's own `Shared` stream (see [Pipeline threads](#pipeline-threads)), so a pool's accounts can arrive over two streams with no order between them. Streams without groups never connect.
- **Transaction statuses on shards.** Each shard also subscribes `transactions_status` for its group accounts (Clock excluded; vote and failed transactions excluded). The plugin sends a transaction's status after every account write of that transaction on the same stream, so the status closes the group of writes that carry its signature. Agave guarantees that order, not the plugin: a batch is committed, which notifies every account write, before it is handed to the single transaction-status thread (agave `v4.2.2` `ledger/src/blockstore_processor.rs`, `rpc/src/transaction_status_service.rs`). Nothing orders writes of other transactions in between. Measured 2026-09-25 with `just txn-probe` (30 min, 280 pools, Yellowstone 15.2.1 on Agave 4.2.2): of 44,434 statuses none arrived before one of its writes or after its slot's `processed` status, and no group went 2 s without one. A 45-minute rerun on an otherwise idle machine put the status 15–133 µs after the group's last write at p50 and under 0.6 ms at p99 for every DEX (max 48 ms). The first run, taken while the same machine was compiling, showed a p99 of 107–169 ms for Whirlpool and DLMM, most likely the probe itself waiting for CPU. In the rerun, a new transaction never wrote a pool before the previous transaction's status arrived, so releasing a group early on the next write would gain nothing.
- **Every request is the full filter set**, because Yellowstone replaces filters on each send. Pubkeys are split into filters of at most `max_pubkeys_per_filter`. Changes are coalesced for `filter_flush_ms` and sent on the open stream; no reconnect.
- **Confirming a filter change.** The plugin sends no acknowledgement, so filter names carry a sequence number and the server tags every update with the names it matched. The first update tagged with a new sequence proves the switch; its slot is the effective slot. A request never carries `ping`: the plugin treats a request with `ping` as a keepalive and leaves its filters unchanged. Server pings are answered with a ping-only request.
- **Clock on every stream.** `SysvarC1ock11111111111111111111111111111111` updates every slot, so every stream subscribes it. It is the heartbeat, the evidence of the stream's current slot (shards pass it on as a `Heartbeat`, which forces transaction groups out in `blocks_meta` mode), and (on the shared stream only) the chain clock readers use.
- **Limits.** Nothing assumes a provider's limits; they are learned from its errors. A group that would push a request over a limit is refused alone (`Rejected`), not the whole stream. Limit errors from the server lower the limits and the stream reconnects. The plugin words account and transaction-status filter limits the same way, so a reported limit is charged to whichever part of the request exceeds it. A key the server does not allow in a filter (`account_include_reject`) is first left out of the transaction-status filter; a transaction that also touches any other key of the stream still sends its status, so the key's writes keep their signature and stay grouped. If the account filter refuses it too, only the groups holding it are rejected.

### Reconnects

The Yellowstone client's own reconnect is off: it injects filters of its own. Each stream owns its reconnects instead:

1. On a drop the stream emits `Down`, its open transaction groups are discarded, and its pools stop being ready.
2. It reconnects with the full filter set and no `from_slot`. Once the new connection's filters are effective, it emits `Gap` with all its keys. The effective slot is the slot of the first update tagged with the new filters, which the new connection delivered itself, so it lies after every write the drop lost (after an unacknowledged filter change, the newest slot this connection delivered, never one from before the drop).
3. The market re-reads only those keys, at `confirmed`, no earlier than that slot plus `settle_slots`, like a first seed. Nothing else is touched. Between `Down` and `Gap` the stream counts as down even once the new connection is up, and its keys are `Syncing` until the re-read (or a newer streamed write) settles them, so a pool is never quotable on state from before the drop. The shared stream's keys are read first, because every pool of the partition waits on them.

There is no `from_slot` replay. The plugin replays only sealed slots and sends them before any live message, so the writes of the slot that was executing when the new filters applied never arrive either way (yellowstone `v15.2.1+solana.4.2.2` `yellowstone-grpc-geyser/src/grpc.rs`), and closing that hole takes the same RPC re-read of every key. A replay would only refresh the cache while the pools are not ready anyway.

Account updates older than `max_message_delay_ms` make a stream reconnect. Slot statuses and pings are not used, because the plugin stamps them with the time it sends them.

`reconnect.max_attempts = 0` retries forever. Only fatal errors (bad credentials, a request the server can never accept) stop the hub.

To exercise this on a live stream, run `just watch-release` and, in another terminal, `sudo scripts/net_fault.sh drop 30` (packets vanish, the stream has to notice) or `sudo scripts/net_fault.sh reset 5` (connections are reset at once). The script cuts only that process's TCP connections, with pf, and restores them after the given seconds. Expect `downs` and `gaps` to rise, `drift` to stay at 0, the stream's pools to be not ready until their re-read, and every pool to become ready again.

## Pipeline threads

`threads.pipeline` (`0` = automatic, see [configuration.md](configuration.md#threads)) sets the pipeline threads, one per partition. Shard `i` feeds partition `i % pipeline`, so a pool's partition follows from its shard. Each partition runs on one OS thread (`pipe-p{i}`, with a single-threaded tokio runtime) and does the whole path from bytes to decoded state for its own pools:

1. **Streams.** Its shard streams and its shared stream run on this thread's runtime: TLS, HTTP/2, gzip and protobuf decoding happen here, and tonic's connection tasks with them.
2. **Engine.** Store, forks, transaction groups, readiness, seeds, repairs and the audit. The engine waits for RPC reads in tasks on this runtime, but the requests themselves run on the gateway's runtime (below).
3. **Decoding.** Every view the engine publishes is decoded here before any reader can load it (see [Decoding](#decoding)).

Metis and Pallas run this path in one pool too (`ROUTER_UPDATE_THREADS`, `PIPELINE_THREADS`); the route search will get a pool of its own. Besides the pipeline, `watch` runs a fixed two-thread runtime (`app`) for startup, the stats line and, in `blocks_meta` mode, the slot feed, and the `rpc` gateway runs its own two-thread runtime (`rpc-io`) for every JSON-RPC request. hyper drives each pooled connection from a task on the runtime that opened it, so a connection opened from a busy pipeline thread would stall every other partition's reads on it; the limiter's and retries' timers would be late the same way.

Partitions share nothing but the read side:

- Each partition has its own shared stream, so Clock, mints and configs are subscribed once per partition. A shared account's `Effective`, seeds and fork state therefore never cross a partition.
- The slot feed (`blocks_meta` mode) sends its slot statuses to every partition, one after the other, so a slow partition delays the others' slot statuses.
- All partitions publish into one lock-free snapshot table, one decoded-state table and one `PoolChanged` feed, and their stats are summed (slots: the newest).
- RPC load stays bounded by the `rpc` gateway's global `max_in_flight` and `max_rps`. `repair_concurrency` applies per partition.

A stream that fails for good (bad credentials, a request the server can never accept) stops its partition, and a panic outside a venue's own guard does too; either way `watch` exits.

## Engine

`market::Engine` owns one partition's state in one task and never waits on RPC. Seeds, repairs and audits run in spawned tasks and report back, so a slow endpoint delays seeds but never the stream. The tick (every `tick_ms`) is polled before stream events, so transaction expiry, repairs and readiness keep running under a steady event flow.

- **One publish per step.** A transaction group, a seed ticket, an audit batch and a fork revert each publish every pool they change once, at the newest slot of their writes, instead of once per account.

- **Transaction groups.** A swap writes the pool and both vaults in separate messages. Applied one by one, a reader could see one vault after the swap and the other before it. The txn probe measured this at 66–86% of pool updates. So a shard write that carries a transaction signature is held until that transaction's status arrives, then the whole group is applied at once and each affected pool gets one `PoolChanged`. Writes without a signature (sysvars, fee payouts and `SlotHistory` when a slot freezes, epoch rewards) and shared-stream writes are applied at once. A group is also known to be complete, and applied, when a later write on the same stream hits one of its keys (`txn_superseded`): Agave notifies every account write of a batch during commit, before the batch releases its locks, so a conflicting transaction cannot write first (agave `v4.3.0` `runtime/src/bank.rs` `commit_transactions`, `accounts-db/src/accounts.rs` `_store_accounts`, `runtime/src/transaction_batch.rs` `Drop`). A group whose status never comes (a transaction touching only keys the status filter had to leave out) is complete once its own stream reports a frozen descendant of its slot: a `processed` status for a slot whose parent chain on that stream reaches the group's slot, or a `confirmed`/`finalized` status at or above it. Agave creates a child bank only after freezing its parent, and a frozen bank takes no more commits (agave `v4.3.0` `runtime/src/bank.rs` `_new_from_parent`, `commit_transactions`), so every write of the ancestors came first. A frozen slot on a sibling fork proves nothing, since Agave replays forks in parallel, and shred statuses prove nothing either. A group below a confirmation that is not its ancestor is on a dead fork; the store drops its writes as late and reads the keys again. These are counted as `txn_orphans`. Another stream's confirmation releases nothing: it says nothing about this stream's writes.

  Without proof, a group is forced out: after `txn_max_hold_ms` of wall time, past 65,536 held writes, or, in `blocks_meta` mode where shards have no slot statuses and so no fork information, once the shard's Clock is two slots ahead. A forced group may miss a write to a key that never arrived, so every account a swap writes in the pools it touches is read again at the group's slot, and those pools are published `Syncing` in the same step that applies the writes; none is ever published ready on half a transaction. Counted as `txn_forced`, with a warning for the time and size limits. The drift audit skips a key while a group holding it is open at or below the audit slot. A dropped stream discards its open groups; the `Gap` re-read brings their state back. Pools whose state spans both streams (DAMM v1, accounts shared by two pools) get no such guarantee.

- **Repair queue.** Keys needing a read are batched into tickets of up to 100 keys, pool and structural accounts first, speculative ones (Whirlpool tick arrays that may not exist) last. At most `repair_concurrency` tickets are in flight, and a key is never read twice at once.
- **Epochs.** A gap bumps each affected key's epoch; a read dispatched before the gap cannot mark the key live. A key whose filter is not effective yet stays `Subscribing` through a gap and is seeded at its own filter's barrier.
- **Drift audit.** Every `audit_interval_ms`, up to 100 live keys are read and compared with what the store held at that slot. A key is compared only once its own stream has confirmed that slot, so every write up to it has arrived. Owner, lamports and data are compared as they are, so a funded but uninitialized address has to match too. A difference replaces the stored copy and is counted.
- **Step times.** Every stream event carries the time its stream handed it over. The `engine` stats line reports, over the last interval and all partitions, how long events queued before the engine took them and how long each event, RPC result and tick (with its closure and readiness passes) held the thread. Every latency histogram (`grpc` lag and blocked, `route` decode, `engine`) reports per interval, not since start.

## Readiness

A pool is **ready** when every dependency is live, every required one exists and has an accepted owner, its streams are up and its closure is complete and verified. Otherwise it reports why, most structural cause first: `Closed`, `Unsubscribable`, `Unverified`, `StreamDown`, `Awaiting`, `Syncing`, `Missing`, `OwnerMismatch`. A missing array disables one pool, not the bot.

An address the System program owns with no data counts as absent even when it holds lamports: anyone can send lamports to a PDA, and the programs treat such an account as uninitialized (seen live on two Whirlpool oracle PDAs).

`market::MarketReader` is the read side for the quote layer:

- `pool_view(pool)`: the pool as the engine last published it: every dependency except the Clock, its readiness, and `cross_stream` when an account a swap writes rides the shared stream (DAMM v1, accounts two pools share), so the view may hold part of a transaction. Reads take no lock: the engine publishes an immutable view per pool (`arc-swap`) after each applied transaction group, seed, audit fix or readiness change.
- `clock()`: the Clock sysvar, never the host clock. It has its own cell, so a Clock update rebuilds no pool view and sends no `PoolChanged`. Every partition writes its own stream's Clock into that cell, so only a newer slot replaces it; a partition behind another, or a rolled-back fork, never moves the clock back.
- `subscribe()`: a `PoolChanged` for each published change to a pool: once per transaction group, when a fork rollback or a dead slot moves an account back, and when the pool's readiness, dependencies or `cross_stream` change without any write (a stream drop, a closure change). A readiness event carries the partition's newest confirmed slot. A view is replaced only when its content, readiness, dependencies or `cross_stream` changed, and every replacement is decoded first and followed by a `PoolChanged`.

## Fork tracking

Updates stream at `processed` so they arrive as early as possible. A `processed` block can still lose to a sibling fork, and Yellowstone sends no "abandoned" message for it. So the store keeps two layers per account:

- **committed**: the newest state from a confirmed slot.
- **pending**: versions from slots that are not confirmed yet.

Slot statuses come from `slot_source`:

- **`slots`**: every stream carries its own slot updates and keeps its own slot tree, because `write_version` is one counter per validator process: a reconnect can land on another node behind a load balancer, so the store only compares it within one `(stream, generation)`.
- **`blocks_meta`**: for providers that refuse `slots`. One extra connection streams confirmed block metadata into one global tree. Dead slots are not reported there, so losing forks are rolled back when the next slot is confirmed rather than when they die.
- **`auto`**: a probe at start picks `slots` if the provider accepts it.

Resolution:

- **When slot C is confirmed**: walk C's parents back to the previous confirmed slot. Pending versions on that chain are promoted to committed. Pending versions from other slots at or below C were on a losing fork and are dropped (`rolled_back`). Only accounts with pending versions at or below C are visited.
- **When a slot is dead**: its pending versions are dropped (`dead_dropped`).
- **A dropped version that was the head** (rolled back or dead) moves the account back to the next pending version or the committed one. That is published like a write: the pools depending on it get a new view and a `PoolChanged`, and a structural or existence change re-derives the closure or readiness.
- **An update for a slot that is already confirmed**: this happens when a transaction group is applied after its slot was confirmed, or when confirmations come over the separate slot-feed connection. The update is never put in pending, because the next confirmation would roll it back. If its slot is on the confirmed canonical chain (the last 512 slots are remembered), it goes straight to committed. Otherwise it is dropped and the account is read again (`late`).
- **Two versions of one slot**: the committed layer remembers whether it came from an RPC read or a stream write. A `confirmed` read is the frozen bank of its slot, so it holds every write of that slot and replaces a streamed write of the same slot; a streamed write never replaces it. Two streamed writes of one slot from the same stream are ordered by connection generation, then `write_version`. From two different streams they cannot be ordered: one is kept and the account is read again (`unordered`). One slot is one bank until Alpenglow ([alpenglow.md](alpenglow.md)).
- **Readers**: `Head` returns the newest pending version, or the committed one. `Committed` returns only confirmed state.
- **RPC reads** are taken at `confirmed`, so they go straight into the committed layer. An account that does not exist is stored as absent at that slot, which is state too.

If a parent link is missing, slots below the hole are promoted as if canonical, `fork_gaps` is counted, and the promoted accounts are re-read. The chain starts at the first `processed` slot a stream sees, and again after every drop; the older statuses resent on connect fill in parents but do not move that start, so the hole below it is not a gap. A drop leaves a hole that the stream's `Gap` re-reads anyway.

**Alpenglow.** The design already follows the official recipe (buffer unconfirmed data, promote the confirmed bank, drop the rest). Yellowstone proto 14 now ships `bank_id`; pending versions and the slot tree have to key on `(slot, bank_id)` instead of `slot`, because one slot may carry more than one bank. [alpenglow.md](alpenglow.md) lists everything the upgrade touches.

## Decoding

Pools are decoded on their partition's pipeline thread, inside the engine's publish: the engine builds a pool's view, hands it to its `route::Decoder` (a `market::ViewSink`), then publishes the view and sends `PoolChanged`. Decoded state is therefore never behind the view a reader loads, and a `PoolChanged` always finds decoded state.

- **Incremental decode.** Per pool the decoder remembers, for every dependency, the update order, owner, lamports and data buffer it last decoded. Any difference counts as a change: a fork rollback moves the order back and RPC seeds share one write version, so "newer" would miss them. Only changed accounts go through `quoter::VenueState::apply`; a dependency that leaves the closure is applied as absent. The state is shared with what was published, so a publish that changes nothing copies nothing.
- **Readiness.** Only `Ready` pools are decoded. `Closed` and `Invalid` pools drop their state.
- **Output.** Each pool's state is published into a lock-free table together with the `PoolView` it was decoded from. `route::QuoteReader::quote` runs on the caller's thread and refuses when the pool is not ready, a dependency failed to decode, or no Clock is known yet. It reads one cell: the readiness, the view and the state decoded from it are published together, and before the market publishes the view, so a quote never pairs a view with state decoded from another. Each publish carries the pool's `Revision`, which moves only when the readiness, a decode error, a panic, the cross-stream flag or the decoded state changed; a republish that changes nothing keeps it. Time-dependent inputs (fees by epoch, activation times) come from the Clock sysvar at quote time, so a Clock update needs no re-decode.
- **Activity.** Right after decoding a pool, the decoder writes the pool's activity bit in the [graph](#graph).
- **Panics.** A venue panic while decoding or quoting is caught: the pool's state is discarded and rebuilt on its next change, and `panics` counts it.
- **Decode time.** Every publish is timed into one lock-free histogram shared by all partitions; the `route` line reports its p50, p99 and max over the last stats interval.
- **Probe.** `watch` exercises the quote path on live state: each stats tick it quotes `route.probe_amount` both ways through every decoded pool on a blocking thread (`QuoteReader::probe`) and logs, per DEX, how many quoted and why the rest refused (`quote probe`), with the sweep time and the slowest quote.

## Graph

`graph::Topology` is the token graph the search runs on. `watch` builds it once from the universe, before the market starts: mints are nodes, every pool gives one edge per direction. Pool mints never change on chain and the universe is fixed for the process, so the structure is never rebuilt; only the activity bits change.

- **Layout.** Mints and pools get dense `u32` ids (`MintId`, `PoolId`), valid for one process. An edge is `PoolId << 1 | b_to_a`, so it needs no table of its own. Outgoing edges are stored per mint, grouped by the mint they lead to: every pool between the same two mints sits in one run, which a search quotes together. Incoming edges are grouped the same way, for closing cycles.
- **Left out.** A pool without two known mints (a Pump bonding curve given by address) or with the same mint on both sides gets no edge. `graph built` counts them as `unplaced` and lists them at debug level.
- **Activity.** One bit per pool: the pool is `Ready`, its DEX has a quoter venue, and every account decoded. The pool's pipeline thread writes the bit right after decoding it, so each bit has one writer and never runs ahead of the decoded state. The bit is a pruning hint only: a search decides from the state it pinned (see [Read contract](#read-contract)), never from the bit.
- **The bit is coarse on purpose.** An active pool can still refuse a quote: a transfer-hook mint, or one direction disabled. A search treats any refused quote as a dead edge for that search.
- **Stats.** `graph built` at start (mints, pools, pairs, edges, unplaced, build time); each stats tick, `graph` (active pools, flips). `just bench graph` measures building the graph and scanning a hub on synthetic power-law universes of 10k and 100k pools.

### Search (next phase)

The read contract, the direct (one-pool) search and the exhaustive depth-first search are in place; the faster search is not implemented yet. The layout above is built for it.

#### Read contract

A search reads through a `route::SearchSession`, taken from `QuoteReader::session`. The topology holds no prices or search state; everything a search derives lives in its session.

- **Clock.** The session takes the Clock once, so fees by epoch and activation times stay the same for every quote in one search.
- **Pins.** The first quote through a pool pins its published state; every later quote of that pool in the session uses the pin, whatever the decoder publishes meanwhile. A new session sees the new state. Pools are pinned at different moments, so the pins are not one chain snapshot; the finalist check and simulation guard what is sent.
- **Pruning.** `SearchSession::active` answers from the pin once a pool is pinned and from the activity bit before, so a bit flip mid-search does not contradict the quotes.
- **Finalist check.** `SearchSession::verify` compares each pool of a candidate against the latest publish: `Unusable` when a pool can no longer be quoted, `Stale` when one changed its `Revision` since it was pinned (or was never quoted in the session) and must be quoted again, `Current` otherwise. `Current` covers state only: it does not quote the amount again, and the Clock is not part of a revision, so a new epoch (a Token-2022 transfer-fee change) or a later time (activation, fee schedules) leaves a pool `Current` while its quote moves. Before a result is acted on, `QuoteReader::requote(path)` prices it again from its first input in a new session, with the current state and Clock. `PoolId`s stay inside the session; a verdict names pools by address and revision, so a later rebuilt topology cannot misread them.

#### Algorithm

- **Direct.** `SearchSession::direct(in, out, amount, max_arrays, allow)` quotes the amount through every pool from `in` to `out` that the request's `allow` filter admits, and returns the highest output with every refusal and its reason. It does not consult the activity bit: a pair has few pools, and quoting each one reports why the others refused. The result names its pool by `EdgeId`, to be checked with `verify` in the same session.
- **Depth first.** `SearchSession::search(query, filter)` tries every single path of up to `max_hops` pools from `from` to the goal: another mint (`Goal::To`) or back to `from` (`Goal::Cycle`, arbitrage). Each leg is quoted exact-in with the previous leg's output, in the session. It is the reference the faster search below is tested against.
- **Path rules.** A path uses a pool once, passes an intermediate mint once, and stops at the goal. No two legs write the same account (`dex::Role::swap_writes` over each pinned view): in one transaction the later swap would run on state its quote did not see. Shared read-only accounts (configs, mints) do not count. A `Filter` admits pools and intermediate mints per request; the activity bit prunes before quoting.
- **Pruning per pair.** With `Query::per_pair = Some(k)` the search quotes every admitted pool of a pair, then continues through only the `k` that pay most; `None` tries them all. The universe is a handful of mints with dozens of pools between the same two, so this, not the graph's breadth, is what a path costs: n₁·n₂·n₃ quotes become n₁ + k·n₂ + k²·n₃. It is a quality-for-speed trade, not a bound. The pools paying most usually lead to the best continuation, and admission runs before ranking so a pool already on the path never takes a runner-up's place. But a later leg sees only the candidates kept: it can share a written account with every one of them, or refuse the larger amounts they pay (liquidity, the array budget) while it would take a smaller one. No `k` rules that out, `k ≥ max_hops` included. `Search::pruned` says candidates were dropped: the result is then the best of the paths tried, and `best: None` does not mean there is no path.
- **Measured.** On the capture of slot 451,259,947 (736 pools, 653 of them in the widest pair), `just bench route` on an Apple laptop: a SOL cycle takes 342 ms exhaustive and 1.6–4.1 ms pruned over two hops, 424 ms and 3.1–12 ms over three; a three-hop pump-token cycle 156 ms and 1.5–7.3 ms (`k` = 1–3). Intersecting write sets by merge instead of scanning cut the pruned cycles by 14–42%. A quote costs about 1–1.5 µs; SOL→USDC stays near 0.8 ms at any `k`, since every pool of the pair is quoted.
- **Choosing `k`.** `just test-universe <capture>` compares every `k` up to `max_hops` against the exhaustive search at 16 random amounts per query (0.01× to 990× of one SOL or 1,000 pump tokens, even over the decades; `ROUTE_SEED` picks them). Over four captures of slots 451,259,947–451,267,240 and two seeds, `k = 1` chose another path in up to 9 of 16 amounts of a pump-token cycle and paid up to 269 bps less, and up to 4 bps less on a SOL cycle; `k = 2` and `k = 3` matched the exhaustive path and payout everywhere. Searches use `k = 2`; the measurement says how often it loses on this universe, not that it never does.
- **Budget.** `max_quotes` caps the quotes one search makes. `Search::exhausted` says the budget ran out first, so `best` is the best found rather than the best there is; no path at all is `best: None` with `exhausted` and `pruned` both false. `max_hops` is a request parameter, not a limit of the design.
- **Result.** The highest final output wins; a cycle may come back at a loss, and whether it pays is the caller's decision. The legs carry their `EdgeId`s for `verify` in the same session.
- Next: a query `(in, out, amount)` runs a hop-layered Bellman-Ford over the graph with real integer exact-in quotes, keeping the best few labels per (depth, mint). Pool uniqueness and the account budget are enforced during the search, not afterwards. Depth is bounded by what the executor can land: 64 account locks per transaction, and the on-chain router's client takes up to 4 hops.
- Arbitrage is the cycle case: when pool u→v changes, search forward from v and close at u; the amount comes from a golden-section search on integers.
- The quoter is exact-in only, so no amount-aware search runs backwards from the output mint.
- Search runs on its own thread pool, apart from the pipeline and route threads, and reads the topology and quotes without locks.

## LiteSVM oracle

`oracle/` is its own Cargo workspace, outside the bot's dependency graph and `cargo deny`: LiteSVM 0.16 and its Solana v4 stack cannot share a lockfile with `domain` (`solana-address` ~2.6 against ^2.8). It never links `quoter`.

1. `just snapshot` (`turk-binary snapshot`) syncs the market like `watch`, then writes up to `--per-dex` ready pools per DEX with the accounts their views hold and the Clock. With `--all` (`just snapshot-universe`, to `oracle/snapshots/universe.json.gz`) it writes every ready pool: the frozen universe the route search is measured and compared on.
2. `just oracle` dumps the deployed bytecode of every program a swap touches (`scripts/dump_programs.py`, public endpoint; `oracle/programs/programs.tsv` records each program's deploy slot and hash), loads it into LiteSVM with mainnet's Rent sysvar and the snapshot's Clock, and runs each pool's swaps both ways at fractions of the input reserve and a fixed ladder of sizes. The instructions come from `oracle/arb-swap-ix`, the previous repo's builders. Accounts a swap passes but no quote reads come from the public endpoint. The user's token accounts are created by the ATA program, so Token-2022 accounts get their extensions. What the program paid, or why it refused, is written to `crates/quoter/src/tests/fixtures/svm/`.
3. `quoter`'s `svm` tests rebuild each pool's closure from the snapshot bytes with `dex::closure`, decode it as the route threads do and require every quote to equal the program's payout, and every refusal to be a refusal.

## Provider probes

`turk-binary probe` checks what the gRPC provider supports: the `slots` filter, Clock streaming, filter-name tagging, ping-only requests, account and transaction-status filter limits and which older slot statuses a new subscription is sent (`slot-backlog`). All checks are read-only. Run it once per provider and set `slot_source` and the limits from its output.

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

Measured 2026-09-24 on the configured provider: `slots` accepted, Clock streamed every slot, filter names tagged on updates, ping-only requests keep the filters, and at least 20,000 pubkeys per filter and 200 filters per request. Measured 2026-09-25: a new subscription is also sent `finalized` statuses, with parents, for about 22 older slots (up to 31 back), mostly after its first `processed` one.

## RPC budget

Designed for a free-tier key (about 10 requests per second, `getProgramAccounts` possibly blocked):

- `max_rps` paces every request. A slot is taken only when its caller is awake to use it, so callers woken late do not start together.
- A 429 is handed back at once instead of being retried inside the Solana client (which retries up to five times, past the limiter: agave `v4.3.0` `rpc-client/src/http_sender.rs`), and pauses every caller for `Retry-After` (500 ms without one). It also widens the request spacing by a quarter; every success narrows it by 1/1024, never past `max_rps`. A pause alone refills the provider's bucket only briefly, so a pace just above its quota would hit a 429 every few seconds (measured 2026-09-26: one every ~3 s at `max_rps = 8`); this way the pace settles just under the quota. The gateway then retries the request like any other transient error, through the limiter.
- Closures never use `getProgramAccounts`; only pool discovery by mint does (use `pools` if the provider blocks it).
- Seeds are one `getMultipleAccounts` per 100 keys. A Whirlpool with `tick_spacing = 1` has about 10,000 possible tick arrays, about 100 reads.
- After a gap only that stream's keys are read again.
- In steady state only the audit reads: one request per `audit_interval_ms`.
