# AGENTS.md

Rust binary for cross-DEX arbitrage on Solana. Rust workspace: binaries under `apps/*`, libraries under `crates/*`.

**This project trades live with real money.** A wrong program ID, account order, fee rate, or rounding direction is a direct loss. The verification rules below are not optional.

## Layout

```text
apps/turk-binary/   # bin: arguments, config, logging, output
crates/domain/      # lib: shared types (Slot, DexKind, AccountUpdate, AccountFilter), no internal deps
crates/dex/         # lib: DEX registry (program IDs, pool filters, mint offsets), no I/O
crates/rpc/         # lib: all JSON-RPC calls
crates/grpc/        # lib: all Yellowstone gRPC streams (sharded hub)
crates/market/      # lib: pool universe resolution, account store, ingestion
crates/quoter/      # lib: account decode and swap quotes per DEX (SDK binds), no I/O
crates/graph/       # lib: token graph built from the universe (mints, pools, edges), per-pool activity bits, no I/O
crates/route/       # lib: decoder run on the pipeline threads (incremental decode, activity bits), quote reader
crates/server/      # lib: HTTP serving (axum): quote API, search thread pool, /health and /ready, graceful shutdown
crates/tx/          # lib: route → router instruction, ATA and WSOL setup/cleanup, v1 account budget; no I/O
docs/               # user docs: architecture, config, DEX table
oracle/             # separate workspace: LiteSVM replay of snapshot swaps on mainnet's deployed programs
onchain/            # separate workspace: the pinocchio router program (router-wire, router-core, programs/router) and programs/short-venue, a never-deployed test venue
```

Every crate's `Cargo.toml`:

```toml
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[lints]
workspace = true
```

Dependency versions live only in the root `Cargo.toml` → `[workspace.dependencies]`. Crates use `foo.workspace = true`.

## Layering rules

- `apps/*` stay thin: argument parsing, config loading, logging setup, printing output. No business logic.
- `crates/*` carry the logic and know nothing about the application: no `clap`, `println!`, or `std::process::exit`.
- Dependencies flow one way: `apps → crates`. A crate never depends on an app; apps never depend on each other.
- Crate-to-crate direction is also one-way and acyclic: `dex, rpc, grpc → domain`, `market → dex, rpc, grpc, domain`, `quoter → dex, domain`, `graph → market, domain`, `route → graph, quoter, market, dex, domain`, `server → route, graph, market, domain, tx`, `tx → domain, router-wire` (`onchain/crates/router-wire`, no dependencies). `domain` depends on no internal crate; `market` never depends on `quoter`, `graph` or `route`.
- **Network access through one door each**: JSON-RPC only via `rpc`, gRPC only via `grpc`, HTTP serving only via `server`. No other crate may pull in `solana-rpc-client`, `yellowstone-grpc-*`, `axum` or `tower-http`; `deny.toml` → `[bans]` enforces this in CI.
- `dex`, `quoter` and `graph` stay pure: no I/O, no async. DEX knowledge lives only in them: `dex` holds what a pool looks like on chain (program IDs, filters, closures), `quoter` how its accounts decode, how a swap is priced, and which accounts its swap instruction takes (`VenueState::swap_window`). `tx` holds no DEX knowledge: it fills a window's user slots and wraps windows in the router's instruction. `market`, `graph` and `route` hold no DEX-specific constants.
- DEX SDK crates and token-program interfaces enter only through `quoter`; `deny.toml` → `[bans]` enforces this. No SDK type appears in `quoter`'s public API.
- When a config key, crate, or DEX is added or changed, `docs/` is updated in the same change.
- Errors: typed errors with `thiserror` in libs, wrapped with `anyhow` in apps.
- If `main.rs` grows past 100 lines, logic has leaked into the wrong place; move it into a crate.

## Sources of truth

In priority order. A higher source overrides a lower one.

1. **On-chain state (RPC)**: the final truth. Is the program deployed, which owner does the account actually have, what size is it.
2. **Program source and IDL (GitHub, default branch)**: account layout, instruction account order, discriminators, fee and price math, rounding direction.
3. **MCP servers** (`.mcp.json`):
   - `solanaMcp`: general Solana. For broad topics, `list_sections` first, then `get_documentation`. For narrow questions or error messages, `Solana_Documentation_Search` or `Solana_Expert__Ask_For_Help`. `program_autofixer` is mandatory whenever on-chain program code is written or changed.
   - `raydium-docs`, `meteora`, `orca-docs`: protocol docs. Search with `search_*`, read a page with `query_docs_filesystem_*`.
4. **llms.txt**: full index of the docs.
   - <https://docs.raydium.io/llms.txt>
   - <https://docs.meteora.ag/llms.txt>
   - <https://docs.orca.so/llms.txt>
5. **Skills**: see "Skills" below. Skill text teaches method; it is not a source of on-chain truth.

The model's training memory is **not** a source. Program IDs, layouts, fee rates, or math are never written "from memory".

### Source repos

| Protocol        | Repo                                                                                                               | Used for                                              |
| --------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------- |
| Raydium CLMM    | [raydium-io/raydium-clmm](https://github.com/raydium-io/raydium-clmm)                                              | program, tick math                                    |
| Raydium AMM v4  | [raydium-io/raydium-amm](https://github.com/raydium-io/raydium-amm)                                                | program                                               |
| Raydium CPMM    | [raydium-io/raydium-cp-swap](https://github.com/raydium-io/raydium-cp-swap)                                        | program, Token-2022                                   |
| Raydium SDK     | [raydium-io/raydium-sdk-V2](https://github.com/raydium-io/raydium-sdk-V2)                                          | reference calculations                                |
| Orca Whirlpools | [orca-so/whirlpools](https://github.com/orca-so/whirlpools)                                                        | program + Rust/TS SDK                                 |
| Meteora DLMM    | [MeteoraAg/dlmm-sdk](https://github.com/MeteoraAg/dlmm-sdk)                                                        | IDL (`idls/dlmm.json`), `commons/` Rust               |
| Meteora DAMM v2 | [MeteoraAg/damm-v2](https://github.com/MeteoraAg/damm-v2), [damm-v2-sdk](https://github.com/MeteoraAg/damm-v2-sdk) | program, SDK                                          |
| Meteora DAMM v1 | [MeteoraAg/damm-v1-sdk](https://github.com/MeteoraAg/damm-v1-sdk)                                                  | legacy                                                |
| Pump.fun        | [pump-fun/pump-public-docs](https://github.com/pump-fun/pump-public-docs)                                          | IDL (`idl/`), `docs/` (bonding curve, PumpSwap, fees) |

The DEX SDKs `quoter` binds are forks under `kaannakiin/*`, pinned by commit in the root `Cargo.toml` with the upstream commit each is based on; `docs/dexes.md` → Quotes records what each fork changes and when upstream was last compared.

Pump.fun has no MCP server or llms.txt. The only official source is this repo: `idl/*.json` and `docs/`. In particular, `docs/BREAKING_*.md` files announce breaking upgrades that add new accounts to the buy/sell instructions.

### Program IDs

Verified: 2026-09-24. Source code or IDL compared against mainnet `getAccountInfo`; all `executable=true`, owner `BPFLoaderUpgradeab1e…`.

| Program            | Mainnet ID                                     | Source                                           |
| ------------------ | ---------------------------------------------- | ------------------------------------------------ |
| Raydium CLMM       | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | `programs/amm/src/lib.rs`                        |
| Raydium AMM v4     | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | `program/src/lib.rs`                             |
| Raydium CPMM       | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | `programs/cp-swap/src/lib.rs`                    |
| Orca Whirlpool     | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`  | `programs/whirlpool/src/lib.rs`                  |
| Meteora DLMM       | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`  | `idls/dlmm.json`, `ts-client/src/dlmm/constants` |
| Meteora DAMM v2    | `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG`  | `programs/cp-amm/src/lib.rs`                     |
| Meteora DAMM v1    | `Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB` | `ts-client/src/amm/constants.ts`                 |
| Pump bonding curve | `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`  | `idl/pump.json`                                  |
| PumpSwap AMM       | `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`  | `idl/pump_amm.json`                              |
| Pump fees          | `pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ`  | `idl/pump_fees.json`                             |

Our own router program (`onchain/`, [docs/router.md](docs/router.md)) is `TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3`, not deployed. Its keypair is the operator's; agents never read it. The keypair `cargo build-sbf` writes to `onchain/target/deploy/` is not the program's key and is never used.

### Reference routers

Verified: 2026-09-29, mainnet `getAccountInfo` and each program's on-chain Anchor IDL.

- **"OKX" means OKX DEX Router v3, `proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u`**: the program Pallas ([okx/dex-solana-binary](https://github.com/okx/dex-solana-binary)) builds swaps for. Closed source; its IDL (`OKX: DEX Router`) is on chain at `8wXL8gQduvMr6pmzhJnbUqsnnegJnmnPiZVPzehLjoeT`.
- [okxlabs/Web3-DEX-Router-Solana-V1](https://github.com/okxlabs/Web3-DEX-Router-Solana-V1) is OKX's **retired v1**, `6m2CDdhRgxpH4WjvdzxAYbGxwdGUz5MziiL5jek2kBma`: its ProgramData is closed and it last ran at slot 436855038. Its source shows method, never what OKX runs today.
- **"Metis" means Jupiter's router `JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4`**, closed source; IDL on chain at `C88XWfp26heEmDkmfSzeXP7Fd7GQJ2j9dDTUsyiZbUTa`.

Raydium repos have separate devnet IDs behind `#[cfg(feature = "devnet")]`. The mainnet ID is the one in the `not(feature = "devnet")` branch.

All programs are upgradeable: layouts and account lists can change. This table is a starting point, not an authority.

### Transaction format

Verified: 2026-09-28. The bot builds **v1** transactions; do not design around address lookup tables.

- v1 (prefix `0x81`) is live on mainnet: `getTransaction` on CPMM swaps at slot ~451386322 returned version 1, and requests without `maxSupportedTransactionVersion: 1` fail with `-32015`.
- v1 has no address lookup tables: every address is inline, at most 64 per transaction (duplicates rejected), in a 4096-byte envelope. SIMD-0596 (draft) would raise the limit to 96.
- v1 has no Compute Budget instructions: compute unit limit, priority fee (total lamports, not micro-lamports per CU), heap size and loaded-data limit are `config` fields of the message.
- An unset v1 config field is **0**, not the legacy default: without `compute_unit_limit` and `loaded_accounts_data_size_limit` the transaction fails before running (`MaxLoadedAccountsDataSizeExceeded`). Both are always set; the requested size is what the cost model charges (SIMD-0553, 32 KiB pages), so it is sized, not maxed.
- Legacy and v0 still work and v0 still supports lookup tables, but they are slated for retirement; nothing new targets them.
- So a route's account count is a hard budget: the router's fixed accounts plus every hop's window, plus the setup and cleanup instructions, must fit in 64.

Sources: <https://solana.com/docs/core/transactions/transaction-pipeline> (v1 limits default to zero), [SIMD-0385](https://github.com/solana-foundation/solana-improvement-documents/blob/main/proposals/0385-transaction-v1.md) (format), [SIMD-0296](https://github.com/solana-foundation/solana-improvement-documents/blob/main/proposals/0296-larger-transactions.md) (4096 bytes), [SIMD-0596](https://github.com/solana-foundation/solana-improvement-documents/blob/main/proposals/0596-increase-txv1-account-lock-limit-to-96.md) (96 accounts, draft), <https://solana.com/upgrades/larger-transaction-sizes>. Recheck them before changing transaction assembly.

## Skills

Installed under `.agents/skills/` in the repo, symlinked into `.claude/skills/` for Claude Code. Versions are in `skills-lock.json`. Install: `npx skills add <repo> --skill <name>`. Exception: `test-audit` is specific to this repo, is not in the lock file, and is edited by hand.

Load the relevant skill before starting the work:

| Task                                                                                   | Skill                                    |
| -------------------------------------------------------------------------------------- | ---------------------------------------- |
| Solana client, building transactions, RPC, PDAs, Token-2022, tests (LiteSVM, Surfpool) | `solana-dev`                             |
| General Rust style, new code, or review                                                | `rust-best-practices`                    |
| Writing, changing, reviewing, or deleting tests                                        | `test-audit`                             |
| Borrow and lifetime errors (E0382, E0597, E0499 …)                                     | `m01-ownership`, `m03-mutability`        |
| `Arc`, `Box`, `Rc`, `Drop`, RAII                                                       | `m02-resource`, `m12-lifecycle`          |
| Generics, traits, `dyn` vs. static dispatch                                            | `m04-zero-cost`                          |
| Newtype, typestate: making invalid states unrepresentable (`Lamports`, `PoolId` …)     | `m05-type-driven`                        |
| `Result`, `thiserror`/`anyhow`, retry and backoff, transient vs. permanent RPC errors  | `m06-error-handling`, `m13-domain-error` |
| tokio, channels, websocket streams, parallel quotes                                    | `m07-concurrency`                        |
| Domain models such as pools, routes, opportunities                                     | `m09-domain`                             |
| Hot path: quote computation, allocation, benchmarks                                    | `m10-performance`                        |
| Crate choice, feature flags, workspace                                                 | `m11-ecosystem`                          |
| Hunting anti-patterns in review                                                        | `m15-anti-pattern`                       |
| Rename, moving functions, extract                                                      | `rust-refactor-helper`                   |

If a skill conflicts with this file, this file wins.

## Verification protocol

**Critical information**: program IDs, PDA seeds, instruction discriminators, instruction account order and writable/signer flags, account layouts and offsets, fee rates and fee computation order, tick/bin/sqrt-price math, rounding direction, token program (SPL Token or Token-2022; transfer fee and hook extensions), mint decimals.

Before critical information is coded:

1. **Two independent sources.** One must be the program source or IDL. The other is RPC, MCP, or the official SDK.
2. **Source record.** Leave `// src: <repo>@<commit-sha> <path>` next to the constant. After an upgrade, this is how you find what to recheck.
3. **Stop on conflict.** If sources disagree, do not guess and do not pick the "closest" one. Report the difference and wait for a human decision.
4. **Math ported verbatim.** Swap quote computation is ported from the program's own code, not derived from a formula. Rounding direction (floor/ceil) and intermediate types (u64, u128, U256) are preserved exactly.
5. **Comparative tests.** Every quote function is tested against real mainnet pool state (LiteSVM or Surfpool with cloned accounts), matching the program's simulation output exactly.
6. **Upgrade tracking.** Before a change that touches a program, check the recent commits of its repo and (for Pump) the `docs/BREAKING_*.md` files.

Information that cannot be verified is marked `TODO(verify)`, and that code path cannot ship to mainnet.

## Live trading safety

- **Keys**: private keys or keypair files never enter the repo, logs, error messages, or agent context. The keypair path is read only from an environment variable.
- **Endpoint secrets**: RPC/gRPC URLs and the `x-token` live in the root `.env` file (gitignored, template `.env.example`). `just` loads it itself. Agents do not read or edit `.env` and never print its contents or any `TB_*` variable; if `watch` is needed, they run `just watch`. `.claude/settings.json` blocks these reads. It is a guardrail, not a sandbox: following the rule is mandatory. No error or log message that leaves the process carries a URL (the `rpc` crate strips URLs from reqwest errors).
- **Agents never send transactions to mainnet** and never run any command with a real keypair. Only a human starts a mainnet transaction.
- **Dry-run is the default mode.** Real submission requires an explicit flag (e.g. `--live`); the config default is never live.
- **Atomic arbitrage.** All legs are in one transaction. `minimum_amount_out` is computed tightly for every swap. A trade below the profit threshold fails instead of landing at a loss.
- **Limits.** Maximum trade size, maximum daily loss, and the kill switch live in config. Code cannot bypass these limits.
- **Arithmetic.** No `f64` on the price and amount path. Integers (u64/u128) with `checked_*`; overflow is an error, not a panic.
- **Test order.** Localnet/LiteSVM first, then mainnet-fork (Surfpool), mainnet with a small amount last. Mainnet is never the first test environment.

## Commands

```sh
just check                     # cargo check --workspace --all-targets
just fmt                       # cargo fmt --all
just lint                      # fmt --check + clippy -D warnings
just test-crate domain         # single-crate tests (nextest)
just test -p domain <filter>   # tests filtered by name
just bench graph               # criterion benches of one crate (heavy: ask first)
just bench route               # route search on the snapshot-universe capture (heavy: ask first)
just bench-ab --base <ref>      # route search bench (--bench clmm: CLMM quotes), <ref> vs working tree, interleaved rounds (heavy: ask first)
just deny                      # cargo-deny
just watch                     # read-only watch with .env (config.toml)
just serve                     # watch plus POST /quote, /swap-instructions, /swap and /health, /ready (unsigned txs; never signs or sends)
just snapshot                  # read-only: ready pools' views + Clock for the oracle
just oracle                    # LiteSVM replay on mainnet's programs → quoter svm fixtures
just snapshot-universe         # read-only: every ready pool's view + Clock, for test-universe and bench route
just test-universe             # pruned vs exhaustive route search on that capture (release)
just find-fee-mints            # read-only, project RPC: Token-2022 mints whose transfer fee changes, to capture as fixtures
just lint-onchain              # router program workspace: fmt --check + clippy -D warnings
just build-onchain             # cargo build-sbf → onchain/target/deploy/router.so
just test-onchain              # router program workspace tests (nextest; ask first like any suite)
just router-replay             # LiteSVM: the CPMM replay corpus's swaps and the router scenarios through the router → tx fixtures (ask first)
just replay-check              # router replay pack rebuilt from the tree, offline; fails unless it reproduces the committed fixtures (CI; ask first)
just publish-oracle-programs   # uploads the program bytecode programs.tsv pins as a GitHub release for CI (human only)
just ci                        # everything CI runs
```

## Tests

- **Read `.agents/skills/test-audit/SKILL.md` before writing, changing, or deleting a test.** No test is added until the four authoring-gate questions in the skill are answered.
- In quote, fee, layout, and offset tests, the expected value comes from a source independent of the code under test: program simulation, a real mainnet fixture, or the official SDK. The port's own output can never be the expected value.
- `crates/dex/src/tests/fixtures/accounts/` holds mainnet bytes; they are never hand-made or deleted. New ones are captured with `scripts/capture_accounts.py`.
- Runner is `cargo-nextest` (`.config/nextest.toml`). Doc tests additionally need `cargo test --doc`.
- Prefer the narrowest run: a single crate (`-p`) or a name filter. The full suite only when needed.
- Tests are written in the crate that holds the logic; tests do not pile up in the app layer.

## Code rules

- Clippy pedantic is on, CI runs with `-D warnings`. `unwrap()` warns: outside tests use `?` or `expect("reason")`.
- `unsafe_code` is forbidden.
- Comments only for "why", only where non-obvious. Never write a comment that restates the code. Exception: the `// src:` source record on on-chain constants is mandatory.
- `just lint` must be clean after every change.

## Codex orchestration

For complex coding tasks, use the `astra-orchestrator` skill when its trigger conditions match. The root agent owns architecture, task boundaries, integration, and final verification. Delegate bounded exploration, implementation, testing, review, or technical research when the skill calls for it. Keep file ownership explicit and avoid delegating trivial work solely for parallelism.

The project-specific source verification, test, and live trading safety rules above apply to every agent. User instructions take precedence over this orchestration policy.
