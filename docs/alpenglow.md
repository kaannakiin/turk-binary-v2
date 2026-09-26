# Alpenglow: what changes for ingestion

Open work, not done yet. Alpenglow replaces TowerBFT and is expected to activate with Agave 4.3 (targeted October 2026). This page lists what it changes for our gRPC ingestion, so the upgrade can be planned in one pass. Every item still has to be checked against the source before it is coded (AGENTS.md → Verification protocol).

Sources read on 2026-09-25: rpcpool/yellowstone-grpc tag `v15.2.1+solana.4.2.2` (commit `2531a87`), its CHANGELOG, and the v16 tag (commit `bfd1d7e`); anza-xyz/agave tag `v4.2.2` (commit `c9c6f32`).

## Versions

| Component                          | We run | Released |
| ---------------------------------- | ------ | -------- |
| Yellowstone plugin (provider side) | 15.2.1 | 16.0.0   |
| `yellowstone-grpc-proto`           | 12.7.0 | 14.0.0   |
| `yellowstone-grpc-client`          | 13.5.1 | 14.0.0   |

## Changes

- **`bank_id` on updates.** Proto 14 adds an optional `bank_id` to account updates and to slot updates. Under Alpenglow one slot can carry more than one bank, so a slot number alone no longer names a fork. Proto 12.7 drops the field without any error: we would silently merge banks.
- **Fork tracking keys.** Pending versions in the store and the slot trees key on `slot` today (`crates/market/src/store.rs`, `crates/market/src/fork.rs`). They have to key on `(slot, bank_id)`, and confirmation has to promote one bank of a slot and drop its siblings.
- **Two Clock writes per slot.** The yellowstone CHANGELOG says the Clock sysvar is written twice per slot under Alpenglow. `Snapshots::set_clock` (`crates/market/src/view.rs`) keeps the current Clock when its slot is not older, so the second write of a slot is ignored. If that write moves `unix_timestamp`, fees by time and activation checks read a stale timestamp. TODO(verify): read the Agave 4.3 source for what the second write changes before fixing the ordering.
- **Clock as heartbeat.** Every stream uses Clock as its heartbeat and replay checkpoint (`docs/architecture.md` → gRPC hub). Two writes per slot are harmless for that, but the per-slot assumptions there need a second look.
- **Client 14 `subscribe` no longer reconnects.** We already disable the client's own reconnect and reconnect per stream ourselves, so this needs no behaviour change, only the API migration.
- **`DiscardBanks` (plugin 16).** The v16 tag adds a way to discard banks that lost. Check how it is delivered and whether it replaces rolling back on the next confirmation.
- **Replay.** Re-check the replay rules (sealed slots only, accounts reduced to the last write per slot, no lifecycle statuses) against plugin 16 with banks in the picture.

## Order of work

1. Read the Agave 4.3 and plugin 16 sources for each item above; record commit and path next to every constant or rule.
2. Upgrade proto and client together; map `bank_id` into `AccountUpdate` and slot events.
3. Re-key the store's pending layer and the slot trees on `(slot, bank_id)`.
4. Fix Clock ordering once the second write is understood.
5. Re-run `just txn-probe` and the fork tests against a provider already on plugin 16.
