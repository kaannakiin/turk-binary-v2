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

**Mint search** asks RPC for the DEX's pool accounts whose first mint equals each configured mint, then keeps pools whose second mint is also configured.

**Address from mint** (Pump bonding curve only): the curve address is derived from the mint, so no search is needed. Curves that have completed (migrated to PumpSwap) are skipped.

## DAMM v1 notes

Meteora lists DAMM v1 as legacy: new pools can no longer be created, but existing pools still trade. Finding and watching pools works like any other DEX. The swap quote needs more care, because the program's source is not public:

- `MeteoraAg/damm-v1-sdk` (formerly `dynamic-amm-sdk`) only has an interface crate. Every instruction body, `swap` included, is empty.
- The deployed programs are newer than the last public commit (source 2025-08-11, pool program 2026-08-27 at slot 442,210,200, dynamic vault `24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi` 2025-12-22).
- Live accounts are larger than the source structs: pool 1387 B (source 875), vault 10240 B (source 1227). The mint offsets sit in the part the source does describe.

So the quote cannot be ported from program source. It has to be checked against the deployed program's own simulated output instead. This is not done here yet.

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
