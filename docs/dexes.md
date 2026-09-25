# Supported DEXes

Use the **config name** in `allowed_dexes` / `blocked_dexes`.

| Config name          | DEX                    | Program ID                                     | Pool account                        | Found by          | Verified |
| -------------------- | ---------------------- | ---------------------------------------------- | ----------------------------------- | ----------------- | -------- |
| `raydium_amm_v4`     | Raydium AMM v4         | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | `AmmInfo` (752 B, no discriminator) | mint search       | yes      |
| `raydium_clmm`       | Raydium CLMM           | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | `PoolState` (1544 B)                | mint search       | yes      |
| `raydium_cpmm`       | Raydium CPMM           | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | `PoolState` (637 B)                 | mint search       | yes      |
| `orca_whirlpool`     | Orca Whirlpools        | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`  | `Whirlpool` (653 B)                 | mint search       | yes      |
| `meteora_dlmm`       | Meteora DLMM           | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`  | `LbPair` (904 B)                    | mint search       | yes      |
| `meteora_damm_v2`    | Meteora DAMM v2        | `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG`  | `Pool` (1112 B)                     | mint search       | yes      |
| `meteora_damm_v1`    | Meteora DAMM v1        | `Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB` | `Pool` (size varies)                | mint search       | yes      |
| `pump_bonding_curve` | Pump.fun bonding curve | `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`  | `BondingCurve`                      | address from mint | yes      |
| `pump_amm`           | PumpSwap               | `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`  | `Pool` (301 B on chain)             | mint search       | yes      |

**Mint search** asks RPC, for every ordered pair of configured mints, for the DEX's pool accounts holding exactly that pair (both mint offsets are filtered). Filtering one mint alone would return every pool with that mint on its side, over a million PumpSwap pools for WSOL.

**Address from mint** (Pump bonding curve only): the curve address is derived from the mint, so no search is needed. Curves that have completed (migrated to PumpSwap) are skipped.

## Dependencies

What `dex::closure` subscribes for each pool, besides the pool itself (see [architecture.md](architecture.md#dependency-closures)). "Swap only" accounts are passed to the swap instruction but not read by its math; `stream_swap_accounts` decides whether they are subscribed.

| DEX                  | Pool-scoped                                                                          | Shared                                                                                         | Swap only                                |
| -------------------- | ------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------- | ---------------------------------------- |
| `raydium_amm_v4`     | coin and pc vaults                                                                   | mints                                                                                          |                                          |
| `raydium_cpmm`       | token vaults                                                                         | `AmmConfig`, mints, Clock                                                                      |                                          |
| `raydium_clmm`       | every tick array the pool bitmap and bitmap extension mark, the extension (optional) | `AmmConfig`, mints, Clock                                                                      | token vaults                             |
| `orca_whirlpool`     | every possible tick-array PDA (optional), Oracle (required on adaptive-fee pools)    | mints, Clock                                                                                   | token vaults, Oracle on static-fee pools |
| `meteora_dlmm`       | every bin array the bitmap and extension mark, the extension (optional)              | mints, Clock                                                                                   | reserves, Oracle                         |
| `meteora_damm_v2`    |                                                                                      | mints, Clock                                                                                   | token vaults                             |
| `meteora_damm_v1`    | the pool's vault LP token accounts                                                   | both vaults, their LP mints and token accounts, the stake account of depeg pools, mints, Clock |                                          |
| `pump_bonding_curve` |                                                                                      | `Global`, `FeeConfig`, mints, Clock                                                            |                                          |
| `pump_amm`           | pool base and quote token accounts                                                   | `GlobalConfig`, `FeeConfig`, mints, Clock                                                      |                                          |

Tick-array PDA seeds encode the start index differently per DEX: CLMM as big-endian `i32`, DLMM as little-endian `i64`, Whirlpool as a decimal string.

Every closure was checked against captured mainnet accounts (`crates/dex/src/tests/fixtures/accounts/`): every derived account exists with an accepted owner, every vault holds the pool's mint on its side, and the Python port that picked the captured arrays agrees with the Rust derivation (417 CLMM tick arrays, 183 DLMM bin arrays, all 362 existing Whirlpool arrays of 2522 possible).

`meteora_damm_v1` was checked on one live pool per curve and depeg type (constant product, plain stable, Marinade, Lido, SPL stake pool) against an independent Python port. The same account set is what `dynamic-amm-quote::compute_quote` reads.

## Source conflicts

Found while verifying and left unresolved:

- Raydium's CPMM docs describe an `AmmConfig.creator_fee_share_rate` field that `raydium-io/raydium-cp-swap@59fb845` does not have. It sits in padding and does not change the swap math.
- `@pump-fun/pump-sdk` derives PumpSwap's config at `["amm_global"]`, which does not exist on chain. The IDL (`create_config`) and `@pump-fun/pump-swap-sdk` use `["global_config"]`, which is live. The code uses `global_config`.

## DAMM v1 notes

Meteora lists DAMM v1 as legacy: new pools can no longer be created, but existing pools still trade. Finding and watching pools works like any other DEX. The swap quote needs more care, because the program's source is not public:

- `MeteoraAg/damm-v1-sdk` (formerly `dynamic-amm-sdk`) only has an interface crate. Every instruction body, `swap` included, is empty.
- The deployed programs are newer than the last public commit (source 2025-08-11, pool program 2026-08-27 at slot 442,210,200, dynamic vault `24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi` 2025-12-22).
- Live accounts are larger than the source structs: pool 1387 B (source 875), vault 10240 B (source 1227). The mint offsets sit in the part the source does describe.

So the quote cannot be ported from program source. The closure rests on two things instead:

- The official quote crate (`dynamic-amm-quote` in the same repo) reads the pool, both vaults, the pool's vault LP accounts, both vault LP mints, both vault token accounts, the Clock and, for depeg pools, one stake account: Marinade's state, Solido's state, or the pool's `stake` for SPL stake pools.
- A quoter over exactly these accounts matched `simulateTransaction` to the lamport on 366 mainnet swaps, 170 of them after the 2026-08-27 redeploy, covering every curve and depeg type (the previous `turk-binary` repo, `damm-v1-onchain-sim.json`). The pool program has not been redeployed since (checked 2026-09-24, slot 450,119,426).

A vault that runs strategies keeps only part of its liquidity in its token account (274 of 730 vaults in that corpus), so that account is always in the closure.

## Pump bonding curve notes

- A curve whose `quote_mint` is zero, or whose account predates that field, is **SOL-paired**, and is matched against the WSOL mint in `mints`. Pump's own docs say so (`docs/PUMP_PROGRAM_README.md`), and legacy curves on mainnet confirm it.
- Accounts written before newer fields existed are shorter. They are read as zero-padded.
- Trading a curve whose account is under 150 bytes will need an `extend_account` instruction first. That only matters once execution exists.

## What "verified" means

Every constant (program ID, discriminator, size, mint offsets) was checked against two independent sources:

1. The program's source code or IDL, pinned to a commit. Each constant's `// src:` comment in `crates/dex` names the repo, commit and file.
2. A live mainnet pool fetched with `getAccountInfo`. Its bytes are kept in `crates/dex/src/tests/fixtures/`, and tests check them against mints the DEX's own API reports.

All values were checked on 2026-09-24. These programs are upgradeable, so re-check a DEX when its repo shows layout changes.

## Shared discriminators

Some DEXes use the same 8-byte account discriminator because their structs share a name:

- Raydium CLMM and CPMM (`PoolState`)
- Meteora DAMM v1, DAMM v2 and PumpSwap (`Pool`)

Pools are always told apart by owner program first, so this is safe.
