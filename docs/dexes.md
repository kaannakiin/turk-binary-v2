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

## Quotes

`quoter` decodes a pool's accounts and computes exact-in quotes. A DEX without a quote yet is counted as `unsupported` by the route threads.

| Config name | Quote | Source of the math                                                                                     | Checked against                                                                             |
| ----------- | ----- | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| `raydium_amm_v4` | yes | `kaannakiin/raydium-amm@e310ed8` (raydium-io/raydium-amm@27f461d + `Option` in `math.rs`): `Calculator::swap_token_amount_base_in`, ceil-divided swap fee, reserves net of `need_take_pnl` | 72 of 72 recorded `simulateTransaction` payouts to the lamport |
| `raydium_cpmm` | yes | `kaannakiin/raydium-cp-swap@8055493` (raydium-io/raydium-cp-swap@244e124 + `Option` in `curve/`): the program's own `PoolState::get_swap_params`, `adjust_creator_fee_rate`, `CurveCalculator::swap_base_input`; pool and config decoded with the program's structs | 1399 of 1399 recorded `simulateTransaction` payouts to the lamport |
| `raydium_clmm` | yes | `kaannakiin/raydium-clmm@1de19c5` (raydium-io/raydium-clmm@51fdba2 + `swap_internal_with_key`, `TickArrayFeed`): the program's `swap_internal` over the tick arrays in the program's own walk order (`get_first_initialized_tick_array`, `next_initialized_tick_array_start_index`), at most `max_arrays` | 218 of 218 recorded `simulateTransaction` payouts to the lamport (incl. fee-on-output and dynamic-fee pools) |
| `orca_whirlpool` | yes | `kaannakiin/whirlpools@536d2da` (client@8.0.0 + `compute_swap` by reference): `orca_whirlpools_core::compute_swap` over the program's own three-array window (`get_start_tick_indexes`), absent arrays as empty (sparse swap), adaptive fee from the Oracle, `trade_enable_timestamp` honoured | 402 of 402 paid `simulateTransaction` cases to the lamport, 108 of 108 refusals refused, 7 of 7 adaptive-fee cases |
| `meteora_damm_v2` | yes | `kaannakiin/damm-v2@0506639` (MeteoraAg/damm-v2@2565067 + curve errors instead of panics): `rust-sdk::quote_exact_in::get_quote`, transfer fee off the input and the output as `process_swap_exact_in` does | 1885 of 1885 paid `simulateTransaction` cases to the lamport, 160 of 160 refusals refused |
| `meteora_dlmm` | yes | `kaannakiin/dlmm-sdk@28e1f83` (MeteoraAg/dlmm-sdk@4eaaeaa + `BinArraySource` quote; RPC helpers behind an `rpc` feature that `quoter` leaves off): `commons::quote_exact_in` over the bin arrays `get_bin_array_indexes_for_swap` picks, at most `max_arrays` | 156 of 156 recorded `simulateTransaction` payouts to the lamport |
| `meteora_damm_v1` | yes | Source port of `dynamic-amm-quote::compute_quote` (damm-v1-sdk@02c66a3): vault share math, constant product via `spl-token-swap`, and the StableSwap curve (`compute_d2`, `compute_y2`, `U192`) from mercurial-finance/stable-swap@140c2e0; pool and vault decoded at the source structs' offsets | 350 of 350 paid `simulateTransaction` cases to the lamport, 16 of 16 refusals refused, every curve and depeg type |
| `pump_amm`  | yes   | `@pump-fun/pump-swap-sdk@1.20.0` (`buyQuoteInput`, `sellBaseInput`, `computeFeesBps`); IDL for layouts | 213 of 215 recorded `simulateTransaction` payouts to the lamport; 125 on-chain fee events |

`dlmm-sdk` carries no license at all and Meteora's DAMM v2 code is under the Meteora Noncommercial Licence; Orca's code is under the Orca License (non-commercial terms since 2025-02-27); `deny.toml` carries it as an accepted exception.

Every fork is pinned by commit in the root `Cargo.toml`. Upstream was compared on 2026-09-25 (fork base to default branch): no change to swap, fee or state code in any of them (Raydium CLMM changed one whitespace line in `states/pool.rs`; CPMM and AMM v4 added lamport-sweeping instructions; DAMM v2 0.2.4 changed config permissions; DLMM and Whirlpools changed SDK and TypeScript code only), so no fork needs a rebase for quoting.

The deployed programs were last upgraded at these slots (checked 2026-09-25, slot 450,363,383): AMM v4 445,763,204, CPMM 445,763,504, CLMM 439,846,317, Whirlpool 440,170,207, DLMM 423,977,638, DAMM v2 445,230,614, DAMM v1 442,210,200, PumpSwap 446,462,733, pump fees 446,465,969. Most simulation cases predate these upgrades (all DLMM cases postdate its last one), so they prove the port against the binary of their day.

Against today's binaries, `oracle/` runs every swap of a live `just snapshot` through the bytecode deployed on mainnet (LiteSVM, see [architecture.md](architecture.md#litesvm-oracle)). Snapshot of slot 450,370,213, programs dumped 2026-09-25:

| Config name       | Pools | Paid, matched to the lamport | Refused by the program, refused by the quote | Out of compute (not compared) |
| ----------------- | ----- | ---------------------------- | -------------------------------------------- | ----------------------------- |
| `raydium_amm_v4`  | 9     | 162                          | 0                                            | 0                             |
| `raydium_cpmm`    | 7     | 126                          | 0                                            | 0                             |
| `raydium_clmm`    | 10    | 129                          | 32                                           | 3                             |
| `orca_whirlpool`  | 10    | 129                          | 45 (12 of them paid 0)                       | 0                             |
| `meteora_dlmm`    | 12    | 89                           | 53                                           | 10                            |
| `meteora_damm_v2` | 12    | 119                          | 69 (9 of them paid 0)                        | 0                             |
| `meteora_damm_v1` | 9     | 100                          | 62                                           | 0                             |
| `pump_amm`        | 12    | 180                          | 6                                            | 0                             |

A swap the program lets through but that pays nothing counts as one the quote must refuse. DAMM v2's deployed binary still panics on zero liquidity (`assert!` in `get_next_sqrt_price_from_input`, upstream 2565067) where the fork returns an error; both refuse.

The array window is a contract between the quote and the transaction that will carry it: the oracle passes every existing tick or bin array from the pool's current position onward in the swap's direction, at most 8 (Whirlpool: the program's three), plus the bitmap extension whenever it exists, and quotes with `max_arrays` set to the arrays passed. A transaction builder has to pass the same arrays.

PumpSwap fees follow the SDK's schedule selection: non-canonical pools pay `flat_fees`; canonical pump pools pay `fee_tiers` by market cap for SOL-like quotes, `stable_fee_tiers` for USDC and `exotic_flat_fees` (or `flat_fees` while unset) for anything else. A pool's own `creator_fee_bps` replaces the creator rate while `GlobalConfig.creator_fee_configurable` is on. Mayhem pools use the fixed 10^15 supply as the market-cap basis. Any set `disable_flags` bit stops both directions, because the bit layout is undocumented.

Token-2022 mints are decoded strictly: every extension must have the length `spl-token-2022-interface@3.1.2` gives it, account-only extensions on a mint are rejected, and an active transfer hook refuses the quote. Transfer fees use the Clock's epoch.

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

So the quote cannot be ported from program source. The closure and the quote rest on two things instead:

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
