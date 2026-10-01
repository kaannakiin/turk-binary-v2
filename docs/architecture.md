# Architecture

## Crates

| Crate              | Job                                                                                           | Talks to the network?      |
| ------------------ | --------------------------------------------------------------------------------------------- | -------------------------- |
| `apps/turk-binary` | CLI, config loading, logging                                                                  | No (uses the crates below) |
| `domain`           | Shared types: `DexKind`, `Slot`, `AccountUpdate`, `AccountFilter`, `ChainClock`, `SwapWindow` | No                         |
| `dex`              | What each DEX looks like on chain: program ID, pool filter, and every account a quote needs   | No                         |
| `rpc`              | Every JSON-RPC call, rate-limited                                                             | Yes, JSON-RPC only         |
| `grpc`             | Every Yellowstone gRPC stream: subscriptions, reconnects, provider probes                     | Yes, gRPC only             |
| `market`           | Picks the pools, keeps every account they depend on subscribed and in sync, answers reads     | Through `rpc` and `grpc`   |
| `quoter`           | Decodes a pool's accounts and computes swap quotes (DEX math and SDK binds); pure             | No                         |
| `graph`            | Token graph built once from the universe: mints, pools, edges, per-pool activity bits         | No                         |
| `route`            | Decodes each pool on the pipeline thread that publishes it; quotes against the decoded state  | No (reads `market`)        |
| `server`           | HTTP serving: the quote API, its search thread pool, readiness, graceful shutdown             | Serves HTTP only           |
| `tx`               | Turns a route's swap windows into the router instruction plus ATA and WSOL setup/cleanup      | No                         |

Dependencies point one way:

```text
turk-binary ──▶ route ──▶ quoter ──▶ dex ──▶ domain
     │            ├─────▶ graph ──▶ market
     │            └─────▶ market
     ├──────────▶ server ──▶ route, graph, market, tx ──▶ domain, router-wire (onchain/)
     ├──────────▶ graph
     └──────────▶ market ──▶ rpc ──┐
                     │  └──▶ grpc ─┤
                     └────▶ dex ───┴──▶ domain
```

`market` never depends on `quoter`: DEX SDKs, with their own Anchor and Solana versions, compile only where quotes are computed.

## One gate per protocol

Only `rpc` may depend on `solana-rpc-client`, only `grpc` on `yellowstone-grpc-*`, and only `server` on `axum` and `tower-http`. `cargo deny` fails CI if any other crate tries. That gives one place for retries, rate limits and error handling.

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
- **Memo.** With the pin and the Clock fixed, a quote is a function of `(edge, amount_in, max_arrays)`, so a flow search that may split answers a repeat from a memo: flow candidates at the same size and allocation moves requote the same legs, and on the capture of `just bench route` about 60% of a split search's quotes are repeats, and the memo cut its time by 60–66% (`just bench-ab`, four interleaved rounds, before the bench capped `max_arrays` at 8). A single path search repeats almost none and measured slower with the memo, so it quotes without one. Refusals are memoized too; a pool that fails to pin is not, since it can be published later. Sequential state transitions (`quote_transition`) are not memoized. The memo holds at most 2¹⁷ quotes, so an exhaustive search does not grow it without bound. `SearchSession::quotes_computed` counts the quotes actually priced.
- **Write sets.** A pool's swap-written accounts are sorted once per publish, on its first pin, and every session that pins the same publish shares the set; concurrent first pins wait for the one build. Against the memo alone, this and the CLMM, DAMM v2 and Whirlpool copy and root changes ([dexes.md](dexes.md)) cut every scenario of `just bench route` at `max_arrays` 8: split searches by 27–40%, single-hop SOL→USDC by 19–51%, cycles by 12–38% (minimum of four interleaved `just bench-ab` rounds, identical outputs). The bench reuses one publish, so the first pin's build after a publish is not in these figures.
- **Windows and token accounts.** A swap window names its tick or bin arrays by address, and a user token account is an associated address: each is a search for a bump off the curve, which cost more than the rest of a candidate's transaction build. A session keeps every window it built by `(edge, arrays used, max_arrays, guard)`, and `tx::TokenAccounts` derives each of one wallet's token accounts once; the server keeps one per request, shared by every candidate it admits. On the universe capture this halved the search threads' busy time over the HTTP matrix ([HTTP measurements](exactin-http-performance.md)).
- **Compute first.** Admission prices a candidate's compute from its windows before it builds any instruction: the build refuses a plan past the budget on that before anything else, so the answer is the same and costs no build.
- **Pruning.** `SearchSession::active` answers from the pin once a pool is pinned and from the activity bit before, so a bit flip mid-search does not contradict the quotes.
- **Finalist check.** `SearchSession::verify` compares each pool of a candidate against the latest publish: `Unusable` when a pool can no longer be quoted, `Stale` when one changed its `Revision` since it was pinned (or was never quoted in the session) and must be quoted again, `Current` otherwise. `Current` covers state only: it does not quote the amount again, and the Clock is not part of a revision, so a new epoch (a Token-2022 transfer-fee change) or a later time (activation, fee schedules) leaves a pool `Current` while its quote moves. Before a result is acted on, `QuoteReader::requote(path)` prices it again from its first input in a new session, with the current state and Clock. `PoolId`s stay inside the session; a verdict names pools by address and revision, so a later rebuilt topology cannot misread them.

#### Algorithm

- **Direct.** `SearchSession::direct(in, out, amount, max_arrays, allow)` quotes the amount through every pool from `in` to `out` that the request's `allow` filter admits, and returns the highest output with every refusal and its reason. It does not consult the activity bit: a pair has few pools, and quoting each one reports why the others refused. The result names its pool by `EdgeId`, to be checked with `verify` in the same session.
- **Depth first.** `SearchSession::search(query, filter)` tries every single path of up to `max_hops` pools from `from` to the goal: another mint (`Goal::To`) or back to `from` (`Goal::Cycle`, arbitrage). Each leg is quoted exact-in with the previous leg's output, in the session. It is the reference the faster search below is tested against.
- **Path rules.** A path uses a pool once, passes an intermediate mint once, and stops at the goal. No two legs write the same account (`dex::Role::swap_writes` over each pinned view): in one transaction the later swap would run on state its quote did not see. Shared read-only accounts (configs, mints) do not count. A `Filter` admits pools and intermediate mints per request; the activity bit prunes before quoting.
- **Pruning per pair.** With `Query::per_pair = Some(k)` the search quotes every admitted pool of a pair, then continues through only the `k` that pay most; `None` tries them all. The universe is a handful of mints with dozens of pools between the same two, so this, not the graph's breadth, is what a path costs: n₁·n₂·n₃ quotes become n₁ + k·n₂ + k²·n₃. It is a quality-for-speed trade, not a bound. The pools paying most usually lead to the best continuation, and admission runs before ranking so a pool already on the path never takes a runner-up's place. But a later leg sees only the candidates kept: it can share a written account with every one of them, or refuse the larger amounts they pay (liquidity, the array budget) while it would take a smaller one. No `k` rules that out, `k ≥ max_hops` included. `Search::pruned` says candidates were dropped: the result is then the best of the paths tried, and `best: None` does not mean there is no path. `SearchSession::search_widening` retries such a search with twice the pools per pair, and last with none dropped, within one `max_quotes` budget for all attempts; a path found pruned is kept, since widening cannot tell whether a better one was dropped.
- **Measured.** On the capture of slot 451,259,947 (736 pools, 653 of them in the widest pair), `just bench route` on an Apple laptop, with `max_arrays` unbounded (the bench now caps it at the server's 8): a SOL cycle takes 342 ms exhaustive and 1.6–4.1 ms pruned over two hops, 424 ms and 3.1–12 ms over three; a three-hop pump-token cycle 156 ms and 1.5–7.3 ms (`k` = 1–3). Intersecting write sets by merge instead of scanning cut the pruned cycles by 14–42%. A quote costs about 1–1.5 µs; SOL→USDC stays near 0.8 ms at any `k`, since every pool of the pair is quoted.
- **Choosing `k`.** `just test-universe <capture>` compares every `k` up to `max_hops` against the exhaustive search at 16 random amounts per query (0.01× to 990× of one SOL or 1,000 pump tokens, even over the decades; `ROUTE_SEED` picks them). Over four captures of slots 451,259,947–451,267,240 and two seeds, `k = 1` chose another path in up to 9 of 16 amounts of a pump-token cycle and paid up to 269 bps less, and up to 4 bps less on a SOL cycle; `k = 2` and `k = 3` matched the exhaustive path and payout everywhere. Searches use `k = 2`; the measurement says how often it loses on this universe, not that it never does.
- **Budget.** `max_quotes` caps the quotes one search makes; a memo answer counts like a priced quote, so the memo changes no result. `Search::exhausted` says the budget ran out first, so `best` is the best found rather than the best there is; no path at all is `best: None` with `exhausted` and `pruned` both false. `max_hops` is a request parameter, not a limit of the design.
- **Result.** The highest final output wins; a cycle may come back at a loss, and whether it pays is the caller's decision. The legs carry their `EdgeId`s for `verify` in the same session.
- **Split/merge.** `search_flow` seeds from DFS, discovers alternative paths at different amounts and with incumbent pools excluded, then refines integer allocation shares. Smaller candidates can seed a split even without a full-size single-route incumbent. Equal directed edges merge and are requoted once on their aggregate input. Each source distributes remaining credits in dependency order. `max_hops` bounds every path; `max_operations` bounds the whole plan. Distinct shared writable states without verified transitions are refused. Supplied repeated CPMM operations use private sequential state; other repeated venue transitions remain unsupported.
- **Chunked split.** With `FlowOptions::chunks = Some(n)` the discovery at fixed sizes is replaced: the order is routed in `n` equal chunks (the first takes the remainder; an order of fewer than `n` units is cut into units), each by a search that prices a leg as the marginal output of its input added to what earlier chunks already send through that edge (`quote(edge, sent + δ) − quote(edge, sent)`). Chunks through one edge execute as one swap of their sum, and the marginal outputs add up to that swap's quote, so the plan's amounts are exact (`just test-universe` requotes every plan in a fresh session and requires it equal, operation by operation); only the greedy choice of paths is approximate, and on three parallel pools it comes within a basis point of a brute force over a 1/200 grid. A chunk's path is admitted only if the plan it joins stays executable: no write shared with another pool, no mint cycle (a pool run both ways always makes one), every mint spending exactly what it earned, `max_hops` and `max_operations` kept, and the caller's `Filter::flow` passing the merged plan. A short allocation refinement (quanta 100 and 10 of 10,000) then moves share between the paths found. The incumbent is the single route, not the current split's plan. Every chunk walks the graph again, but the memo answers each edge an earlier chunk did not move, so in this mode `max_quotes` bounds the quotes computed, not the quote calls: the session holds a ceiling that the single-route search, every chunk and the refinement check before each quote they may price, so it is never passed. `FlowSearch::quotes` counts the calls, memo answers included, and `FlowSearch::computed` the quotes priced. If the budget or deadline stops it early, the paths found carry the whole order in proportion to what they carried, or the incumbent stands; a partial order is never returned. The server routes in `route::SPLIT_CHUNKS` (8) chunks ([measurements](exactin-performance.md#chunked-split-under-admission)). Its large orders send big chunks into thin pools, whose swaps cross many ticks or bins; the compute budget counts what each swap walks, so such a plan is admitted only with the compute it will spend, and `just router-split-replay` runs the plans the server builds through the router in `LiteSVM`.
- **Layered alternative.** `search_layered` retains complete path histories per frontier, not only the largest amount at a mint/depth. It is a benchmark alternative; capped cyclic searches differ from DFS, so it is not promoted. See [current comparison](exactin-performance.md). Linear router v2 remains supported; flow v1 allows up to 16 operations subject to transaction budgets.
- Arbitrage is the cycle case: when pool u→v changes, search forward from v and close at u; the amount comes from a golden-section search on integers.
- The quoter is exact-in only, so no amount-aware search runs backwards from the output mint.
- Search runs on its own thread pool (`search-{i}`, see [HTTP API](#search-threads)), apart from the pipeline and route threads, and reads the topology and quotes without locks.

## HTTP API

`serve` runs `watch` and answers on two addresses, like the Metis and OKX (Pallas) binaries: the quote API on `server.api_addr` (default `127.0.0.1:8080`) and the probes on `server.ops_addr` (default `127.0.0.1:9100`), so probes and metrics stay off the API port. Both are bound before the universe is resolved: a taken port fails at once, and requests during the long startup get an answer (`503`) instead of a hung connection.

It prices operation plans (`/quote`) and builds router instructions and unsigned v1 transactions (`/swap-instructions`, `/swap`). It never signs or sends. A swap either searches from `quoteRequest` or validates a returned `quoteResponse`. Returned amounts must conserve credits across the entire graph; current pool state supplies the swap windows, while the router enforces the client's thresholds on chain.

The router builds Raydium AMM v4, CPMM and CLMM, Orca Whirlpool and Meteora DLMM hops (`tx::supports`), and every endpoint routes through those only: a quote is a route the swap endpoints can build. AMM v4 uses `SwapBaseInV2` and SPL Token accounts; its Token-2022 pools are refused. CPMM retains its Token-2022 transfer-fee handling. All three endpoints admit a candidate with the same route planner and v1 transaction budgets; `/quote` builds it for a stand-in wallet, since who signs changes the keys of the accounts, not their count, and answers a cycle that loses instead of refusing it.

```text
HTTP (axum, `app` runtime)            search threads (`search-{i}`)
  parse + validate ─▶ admit ──────────▶ open SearchSession ─▶ search_flow ─▶ requote_flow ─▶ reply
       │  400          │ 503 OVERLOADED      (state pinned from here, not from arrival)
       └───────────────┴── wait ≤ timeout_ms ─▶ 504 TIMEOUT
```

### `POST /quote`

```json
{
  "fromTokenAddress": "So11111111111111111111111111111111111111112",
  "toTokenAddress": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  "amount": "1000000000"
}
```

| Field                   | Required | Meaning                                                                                                                                                                                |
| ----------------------- | -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fromTokenAddress`      | yes      | Input mint, base58.                                                                                                                                                                    |
| `toTokenAddress`        | yes      | Output mint. The same as the input only with `enableCyclicArbitrage`.                                                                                                                  |
| `amount`                | yes      | Exact input in base units, as a string of digits: no sign, no decimals, below 2^64.                                                                                                    |
| `userWalletAddress`     | no       | Wallet address when the request is embedded in a transaction request; a quote-only request may omit it.                                                                                |
| `slippagePercent`       | no       | Decimal percentage, defaulting to `swap.default_slippage_bps` converted to percent; at most two decimals and below 100.                                                                |
| `enableCyclicArbitrage` | no       | `true` searches a cycle back to the input mint (at least 2 hops). It may come back at a loss: that is the caller's call.                                                               |
| `maxHops`               | no       | Pools a route may pass, `quote.default_max_hops` when absent, at most `quote.max_hops`.                                                                                                |
| `dexIds`                | no       | Comma-separated DEX program IDs. Empty: every loaded DEX.                                                                                                                              |
| `excludedDexIds`        | no       | Comma-separated DEX program IDs that are never used; exclusion wins over `dexIds`.                                                                                                     |
| `allowedPools`          | no       | Pool allowlist. Missing or `null` means no filter; `[]` admits no pool. Unknown or unloaded pools are ignored.                                                                         |
| `directRoute`           | no       | Restricts the search to one pool and sets the route hop limit to one.                                                                                                                  |
| `singleRouteOnly`       | no       | Forbids parallel route branches while allowing a multi-hop route.                                                                                                                      |
| `singlePoolPerHop`      | no       | Allows at most one pool for each directed mint pair in a hop.                                                                                                                          |
| `uniqueDexIds`          | no       | Comma-separated program IDs eligible for the cycle-wide unique-DEX rule.                                                                                                               |
| `enableUniqueDex`       | no       | Defaults to `true`; when false, uniqueness is disabled after IDs are still validated.                                                                                                  |
| `maxAccounts`           | no       | Most distinct addresses the returned instructions may name, the wallet included: 1 to 64, default 64 (a v1 transaction holds 64). Lower it to leave room for instructions of your own. |

Unknown fields are refused, so legacy names such as `dexes`, `excludeDexes` and `slippageBps` are rejected. The answer:

```json
{
  "fromTokenAddress": "So111…",
  "toTokenAddress": "EPjF…",
  "fromTokenAmount": "1000000000",
  "toTokenAmount": "33540506",
  "otherAmountThreshold": "33372803",
  "slippagePercent": "0.5",
  "contextSlot": 450370213,
  "crossStream": false,
  "search": { "pruned": false, "exhausted": false, "quotes": 7 },
  "maxAccounts": 64,
  "slots": ["So111…", "EPjF…"],
  "operations": [
    {
      "sourceSlot": 0,
      "destinationSlot": 1,
      "inputShare": { "numerator": "1", "denominator": "1" },
      "dependencies": [],
      "poolAddress": "…",
      "dex": "raydium_cpmm",
      "fromTokenAddress": "So111…",
      "toTokenAddress": "EPjF…",
      "fromTokenAmount": "1000000000",
      "toTokenAmount": "33540506"
    }
  ]
}
```

- **Amounts** are the winning path priced again (`requote`) in a new session: the newest decoded state and Clock. The search compared paths on pins taken at different moments; the answer is not one of those. That session then `verify`s the path, since a pool pinned early can change or become unusable before the last is quoted. This catches what changed while the answer was prepared; it is not one chain snapshot and promises nothing about execution.
- **Freshness** is checked when a search thread takes the request, so a feed that stalled while the request waited is caught too. It is the whole-feed signal: a stalled Clock means the streams stopped, however ready their pools still look. Traffic policy (the pool share, draining) stays with `/ready` and is not applied per quote.
- **`otherAmountThreshold`** is `toTokenAmount` less the requested `slippagePercent`, rounded down (`tx::min_out`, u128): the least the router will accept on chain.
- **`contextSlot`** is the slot of the Clock that requote used. It says when the price held, not that it will hold when a transaction lands.
- **`search`** reports exploration quality separately from freshness. `pruned` covers per-pair and heuristic split candidate/allocation limits; no global optimum is promised. A split search that finds no single path at the full amount or at a half, quarter or eighth of it answers `pruned: true`: splits into smaller parts were never tried. `exhausted` marks incomplete exploration from the quote budget or cancellation/deadline. Optional `timedOut: true` distinguishes deadline/cancellation termination. A valid incumbent is retained when exploration stops.
- **`crossStream`** means some account a swap writes rides the shared stream, so the state priced may hold part of a transaction.
- **`maxAccounts`** is the limit the route was admitted under. A swap built from this quote is held to it: when the current state needs more tick or bin arrays than were priced and the instructions outgrow it, the answer is `TOO_MANY_ACCOUNTS`.
- Pools and mints are addresses; the graph's `PoolId`/`EdgeId` never leave the process.

Errors are `{"error":{"code","message"}}`, `code` being the stable part:

| Status | `code`            | When                                                                                                                                                                                                                    |
| ------ | ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 400    | `INVALID_REQUEST` | The body does not parse, a field is malformed or unknown, `maxHops` or `maxAccounts` is out of range, or the mints contradict `enableCyclicArbitrage`.                                                                  |
| 422    | `UNKNOWN_MINT`    | A mint no watched pool trades.                                                                                                                                                                                          |
| 422    | `NO_ROUTE`        | No path; `error.search` says whether pruning or the budget may have hidden one.                                                                                                                                         |
| 503    | `NOT_READY`       | The engine has not started yet, or has no Clock.                                                                                                                                                                        |
| 503    | `STALE_DATA`      | The Clock has not moved for `ready.max_clock_stall_ms`: the feed stalled, however ready its pools still look.                                                                                                           |
| 503    | `OVERLOADED`      | Every search thread is busy and `quote.max_queued` searches wait. Answered at once, with `Retry-After: 1`.                                                                                                              |
| 503    | `ROUTE_CHANGED`   | A pool of the winning path failed its requote, the route priced again no longer fits `maxAccounts` or the transaction budgets, or `verify` after it found one unusable or published again. Asking again searches again. |
| 504    | `TIMEOUT`         | The search did not finish within `quote.timeout_ms`, queue time included.                                                                                                                                               |
| 500    | `INTERNAL`        | The search panicked. The thread survives.                                                                                                                                                                               |

### `POST /swap-instructions` and `POST /swap`

```json
{
  "userWalletAddress": "…",
  "quoteRequest": {
    "fromTokenAddress": "So111…",
    "toTokenAddress": "EPjF…",
    "amount": "1000000000"
  }
}
```

| Field                 | Required    | Meaning                                                                                                                                                                                       |
| --------------------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `userWalletAddress`   | yes         | The wallet that signs, pays the fee and owns the token accounts.                                                                                                                              |
| `quoteRequest`        | one of them | A `/quote` body: the route is searched in this request, over the venues the router supports only.                                                                                             |
| `quoteResponse`       | one of them | A `/quote` answer sent back: validate its operation graph and amounts, then reprice to obtain current swap windows; preserve the client's minimum output.                                     |
| `wrapAndUnwrapSol`    | no          | Default `true`: a SOL input is wrapped into the user's WSOL account first, a SOL output is paid into it, and the account is closed after, which unwraps WSOL the user already held there too. |
| `priorityFeeLamports` | no          | The v1 transaction's priority fee, total lamports (not per compute unit). Default 0.                                                                                                          |

A `quoteResponse`'s `search` and `crossStream` describe its original search and are echoed back as sent, or omitted when absent. Repricing verifies current windows and pool revisions without replacing the client's amounts or threshold. A `quoteRequest` searches pools admitted by its DEX/pool filters and supported by the router. Full transaction resource admission runs before a candidate replaces the incumbent.

A `quoteResponse` is refused with `QUOTE_EXPIRED` when its slot exceeds the configured age. `QUOTE_MISMATCH` covers invalid pool/venue identities, dependencies, slot mint identities, allocation ratios, amount conservation, excessive path depth and branched cycles. Slots 0/1 represent source/final credits. Each operation spends its fraction of the remaining source credit; the final branch consumes the remainder. Dependencies list earlier producers of that source. Existing user balances never contribute intermediate credits. Swap accounts always come from market state.

`/swap-instructions` answers the route and its instructions, for the client to put in its own transaction:

```json
{
  "quote": { "…": "as /quote" },
  "setupInstructions": [
    {
      "programId": "…",
      "accounts": [{ "pubkey": "…", "isSigner": true, "isWritable": true }],
      "data": "<base64>"
    }
  ],
  "swapInstruction": {
    "programId": "TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3",
    "accounts": [],
    "data": "<base64>"
  },
  "cleanupInstructions": [],
  "computeUnitLimit": 210000,
  "loadedAccountsDataSizeLimit": 2031616,
  "priorityFeeLamports": 0
}
```

- **Setup** creates every token account the route pays into (`CreateIdempotent`) and, for a SOL input, wraps it. **Cleanup** closes the WSOL account: all of its balance comes back as SOL, including WSOL the user held before the swap.
- **The user's own token accounts are not checked, by design.** How the user configured them is theirs: a source with `CpiGuard` locked, a destination requiring memos or refusing non-confidential credits, or a frozen account makes the transaction fail on chain, and neither the server nor the router reads them first. OKX v3 and Metis do not either (their IDLs have no such error). Pool-side states, which the market streams, are refused at quote time ([dexes.md](dexes.md) → Quotes).
- **The swap** is the router's `route` instruction ([router.md](router.md)): the router checks every hop's real balance change and that the route paid at least `otherAmountThreshold`.
- **`computeUnitLimit`** is `tx::compute_unit_limit`: a budget per hop plus a flat allowance for setup. CPMM and AMM v4 hops take a fixed budget. CLMM, Whirlpool and DLMM hops are budgeted by how far their quote walks (`domain::Walk`): a rate per initialized tick crossed (per bin for DLMM), per step of a dynamic- or adaptive-fee loop, and per array, plus a flat amount for a pool that has a fee loop at all, per Token-2022 side, and more per side whose mint carries a transfer fee extension (`domain::TokenSide::has_transfer_fee`, which the quoter reads from the mint it already decodes; a 0 bps fee counts, since Token-2022 still processes the extension). A Whirlpool swap that crosses no tick spent 70,000 to 72,500 units with one transfer-fee side, 36,100 to 51,800 with SPL tokens. The rates are fitted to what the router spent in `LiteSVM` on 2,867 one-hop swaps of 72 pools under the largest compute limit (`crates/tx/src/tests/fixtures/router_compute.json`, `just router-compute-replay`) with 15% to spare; the replay measures each swap outside the budget it carries, so a swap the budget would underfund is still measured. A fee loop steps only while its volatility accumulator is below its maximum, so the quoter caps those steps at `2·⌈max_volatility_accumulator / 10,000⌉ + 1` from the pool's own constants; a long swap through such a pool is not budgeted as if every tick it passes cost a step. A route whose sum exceeds 1,400,000 is refused as `TOO_MUCH_COMPUTE` even when it would run in less. There are no Compute Budget instructions: a v1 transaction carries the limit and the priority fee in its config (AGENTS.md → Transaction format).
- **`loadedAccountsDataSizeLimit`** must be set too: a v1 transaction that leaves it unset may load 0 bytes and fails with `MaxLoadedAccountsDataSizeExceeded` before any instruction runs. `tx` sizes it from the programs and accounts of the transaction (`crates/tx/src/budget.rs`, see [open-work.md](open-work.md)); a program without a known size is refused.
- **Thresholds** are one rule for the search and the build: a searched route is admitted only if the route's and every operation's quoted output less `slippagePercent` stays above zero and the route builds with those thresholds, so a candidate the build would refuse never displaces one it accepts.
- **A cycle** (the route ends on the mint it spends) is built only when `otherAmountThreshold` exceeds the input, the router's own precondition; otherwise `UNPROFITABLE_CYCLE`. A searched cycle is admitted only past that check; when every cycle found fails it, the answer is `UNPROFITABLE_CYCLE` rather than `NO_ROUTE`. A `quoteResponse`'s threshold is never raised to make one pass.
- A route must fit a v1 transaction: at most 64 accounts (`TOO_MANY_ACCOUNTS`), 4096 bytes (`TRANSACTION_TOO_LARGE`) and 64 MiB of loaded account data (`TOO_MUCH_ACCOUNT_DATA`).

`/swap` answers the same route as one unsigned v1 transaction on the newest blockhash, `"transaction": "<base64>"` with its `lastValidBlockHeight`; every signature slot is zero for the wallet to fill. The blockhash comes from `getLatestBlockhash` at `confirmed`, refreshed every `swap.blockhash_refresh_ms`; `/swap` answers `NO_BLOCKHASH` while none newer than `swap.max_blockhash_age_ms` is known.

| Status | `code`                  | When                                                                              |
| ------ | ----------------------- | --------------------------------------------------------------------------------- |
| 400    | `INVALID_REQUEST`       | Neither or both of `quoteRequest` and `quoteResponse`, or a malformed field.      |
| 422    | `QUOTE_EXPIRED`         | The `quoteResponse` is older than `swap.max_quote_age_slots`.                     |
| 422    | `QUOTE_MISMATCH`        | The `quoteResponse` is not a route this market can build.                         |
| 422    | `CANNOT_SWAP`           | A pool of the route has no swap accounts (not ready, or no window for its venue). |
| 422    | `UNSUPPORTED_VENUE`     | A leg's venue has no router adapter.                                              |
| 422    | `TOO_MANY_ACCOUNTS`     | The route needs more than 64 accounts.                                            |
| 422    | `TRANSACTION_TOO_LARGE` | The transaction is over 4096 bytes.                                               |
| 503    | `NO_BLOCKHASH`          | `/swap` only: no recent blockhash yet.                                            |

The other codes are `/quote`'s.

### Search threads

Searches run on dedicated threads (`[threads] search`). At most one runs per thread and `quote.max_queued` wait. A session opens when work starts, so queued requests pin no state. Deadline starts before queue admission; dropping the HTTP request sets cooperative cancellation. DFS checks before quoting and allocation refinement checks between candidates. An individual SDK quote cannot be preempted; the worker retains admission until it returns. `quote.max_quotes` bounds quote calls independently of the deadline. Queued work whose caller left is dropped unrun.

### `/health` and `/ready`

| Endpoint  | Answers                                                                                                    |
| --------- | ---------------------------------------------------------------------------------------------------------- |
| `/health` | `200 {"status":"ok"}` whenever the process can answer, during startup and shutdown too. For liveness.      |
| `/ready`  | `200` when the engine can serve quotes, `503` otherwise, with a body saying why. For routing traffic here. |

Once the engine runs, `/ready` needs:

- a Clock sysvar whose slot moved within `ready.max_clock_stall_ms`. The Clock carries no host time, so the market notes the host time whenever a newer slot replaces it (`MarketReader::clock_advanced_at`); a partition behind another or a rolled-back fork moves neither. A stalled stream stops the Clock;
- enough ready pools among the **eligible** ones: ready, or not ready for a reason that clears by itself. `Unverified`, `Invalid`, `Unsubscribable` and `Closed` pools never become ready on their own, so they are left out; `Missing` and `OwnerMismatch` stay in, since with real money an unclear case counts against readiness. `/ready` first turns 200 at `ready.startup_percent` of them, so a service still seeding takes no traffic. From then on it fails again only below `ready.floor_percent`: a pool or a stream shard dropping out is a question for each request (does this route have data?), not for the whole service.

```json
{
  "ready": false,
  "phase": "serving",
  "reasons": ["CLOCK_STALLED"],
  "slot": 371234567,
  "slotAgeMs": 12250,
  "readyPools": 812,
  "eligiblePools": 820,
  "totalPools": 840
}
```

`reasons` holds `STARTING`, `DRAINING`, `NO_CLOCK`, `CLOCK_STALLED`, `TOO_FEW_READY_POOLS`; `phase` is `starting`, `serving`, `draining` or `stopping`. Unknown paths and methods answer `NOT_FOUND` or `METHOD_NOT_ALLOWED` on both addresses. Every response carries `x-request-id`: the client's own if it sent one, otherwise a new UUID, and the request's tracing span records it.

### Connections

Both listeners run their own accept loop on hyper (`hyper-util`), not `axum::serve`: that spawns every connection as a task of its own, and dropping the server leaves those tasks running, so it cannot promise that no request outlives it. Here every connection is a task in a `JoinSet` the server owns. Request headers must arrive within `read_timeout_ms`, and on the API so must the body (the search timeout starts only once the body is read); a client that stops sending is cut off instead of holding its connection and task for good.

### Shutdown

Ctrl-c or `SIGTERM` first **drains**: `/ready` answers 503 (`DRAINING`) while the API keeps serving for `drain_delay_ms`, so a load balancer moves traffic away. Then it **stops**, within one `shutdown_timeout_ms` deadline:

1. Both listeners stop accepting; open connections are told to finish (hyper's graceful shutdown) and idle ones close.
2. At the deadline, connections still open are aborted and their tasks awaited, so `run` returns only when no request task is left.
3. The search pool refuses new searches; queued ones whose caller is gone are dropped unrun, and the server waits for running ones until the same deadline. One still running then is logged and left to finish unread: a thread cannot be stopped safely, and nothing reads its answer.
4. The engine stops last.

A market failure skips the drain delay, since its state no longer updates, then takes the same stop path and the process exits with the error.

## LiteSVM oracle

`oracle/` is its own Cargo workspace, outside the bot's dependency graph and `cargo deny`: LiteSVM 0.17 and its Solana v4 stack resolve `solana-address` 2.7, which cannot share a lockfile with `domain` (^2.8). It never links `quoter`.

1. `just snapshot` (`turk-binary snapshot`) syncs the market like `watch`, then writes up to `--per-dex` ready pools per DEX with the accounts their views hold and the Clock. With `--all` (`just snapshot-universe`, to `oracle/snapshots/universe.json.gz`) it writes every ready pool: the frozen universe the route search is measured and compared on.
2. `just oracle` dumps the deployed bytecode of every program a swap touches (`scripts/dump_programs.py`, public endpoint; `oracle/programs/programs.tsv` records each program's deploy slot and hash), loads it into LiteSVM with mainnet's Rent sysvar and the snapshot's Clock, and runs each pool's swaps both ways at fractions of the input reserve and a fixed ladder of sizes. The instructions come from `oracle/arb-swap-ix`, the previous repo's builders. Accounts a swap passes but no quote reads come from the public endpoint. The user's token accounts are created by the ATA program, so Token-2022 accounts get their extensions. What the program paid, or why it refused, is written to `crates/quoter/src/tests/fixtures/svm/`.
3. `quoter`'s `svm` tests rebuild each pool's closure from the snapshot bytes with `dex::closure`, decode it as the route threads do and require every quote to equal the program's payout, and every refusal to be a refusal.
4. `just router-replay` runs the router itself: `oracle router` and `oracle router-scenarios`, described in [router.md](router.md) → Status.

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
- `serve` adds one `getLatestBlockhash` per `swap.blockhash_refresh_ms` (default 2 s).
