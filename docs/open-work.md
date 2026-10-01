# Open work

What is known to be missing, with why it matters. An item leaves this page in the change that does it.

## A live feed of what a transaction loads

`tx` sizes a v1 transaction's `loadedAccountsDataSizeLimit` from a table in `crates/tx/src/budget.rs`: each program's programdata size measured on mainnet once (AMM v4 measured at slot 451595266), half again for growth, plus a flat 10 KiB per other account. It is safe (too small only fails the transaction, it loses nothing) but it goes stale: a program upgrade can outgrow it, and a venue added without a row is refused (`UNSUPPORTED_VENUE`).

The budget should come from a structure fed continuously instead, as Pallas serves it from its own endpoint:

- subscribe the programdata account of every program a route can invoke or pass (venue programs, both token programs, the ATA program, the router) through the gRPC hub, and keep their sizes current;
- take every other account's data length from the market's own views, which already hold them;
- compute the limit exactly per SIMD-0186 (data length plus 64 bytes per account, programdata of LoaderV3 programs), with headroom for the accounts the setup creates, rounded up to 32 KiB pages (SIMD-0553).

The compute unit limit is a per-hop model fitted to replayed swaps (`just router-compute-replay`); it covers only what the replays reached. DLMM windows of four to eight bin arrays are refused because no measured swap took more than three; a swap starting at the edge of its array reaches a fourth in 142 bins, which the budget would admit, so that is the first to measure, and a swap whose estimate alone passes 1.4 million is refused unmeasured. Simulating the route before sending would replace the estimate with what it spends.

The AMM v4/CPMM matrix snapshots contain market closure accounts at one slot, but CPMM observation accounts are fetched when `oracle router-matrix` first runs and cached with their bytes in the replay fixture. Later replay runs are deterministic from that fixture. A future capture should fetch observation accounts alongside pool state at the same slot; the current cached observation bytes may come from a later slot.

## Router behaviour not yet proven by execution

`just router-replay` runs the router's own paths on mainnet's bytecode ([router.md](router.md) → Status lists them). Two refusals stay unreached because an earlier check always fires first: the venue program ID in `hop::invoke` (the adapter's window check precedes it) and an unsigned `initialize` payer (the payer there also pays the fee). The venues without an adapter are not reached at all.

## Transfer-hook mints

`quoter` refuses any mint whose transfer hook names a program (`QuoteError::TransferHook`); a hook extension with no program routes like any other mint (scenario `hook_extension_without_a_program`). Checked 2026-09-29 from source and mainnet:

- **What a hook can do.** Token-2022 runs the hook inside `TransferChecked`, after the balances have moved, with source, mint, destination and authority read-only and unsigned (solana-program/token-2022@f4a1c94 `program/src/processor.rs` `process_transfer`; solana-program/transfer-hook@ec70632 `interface/src/instruction.rs` `execute`). It cannot change the amount or move that transfer's tokens; it can only fail the transfer or act on the extra accounts it is given. Plain `Transfer` fails for a hook mint (`MintRequiredForTransfer`). So the router's balance checks stay sound; what a hook adds is failure, compute and accounts.
- **Resolving it off chain.** The extra accounts live in the hook program's PDA `["extra-account-metas", mint]` as a TLV list of 35-byte `ExtraAccountMeta` entries (fixed keys, PDAs seeded by literals, instruction data, account keys or account data). A client resolves them from on-chain data (`spl_transfer_hook_interface::offchain::add_extra_account_metas_for_execute`), so the market can hold them in memory like any pool account. Seeds may depend on the transfer amount and the real source and destination, so resolution must use the hop's own accounts and amount.
- **CPI depth.** Mainnet allows 5 instruction levels (anza-xyz/agave@fd02ff5 `program-runtime/src/execution_budget.rs` `MAX_INSTRUCTION_STACK_DEPTH`; the SIMD-0268 feature `6TkHkRmP7JZy1fdM6fg5uXn76wChQBWGokHBJzrLB3mj` has no account at slot 451587654). Router → venue → Token-2022 → hook is 4, so a hook may make one CPI of its own and no deeper chain.
- **Which venues can pass hook accounts:**

| Venue              | Hook mints in pools                                                                                                                                                                                                                                                                                                                                            | Swap passes hook accounts                                                                                                          |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Raydium AMM v4     | no (SPL Token only)                                                                                                                                                                                                                                                                                                                                            | no                                                                                                                                 |
| Raydium CPMM, CLMM | only through an admin `SupportMintAssociated` (the allowlist in `is_supported_mint` excludes TransferHook); none found among 2459 listed mints                                                                                                                                                                                                                 | no: `transfer_checked` with the four base accounts (raydium-cp-swap@59fb845 `utils/token.rs`)                                      |
| Orca Whirlpool     | with a `TokenBadge`; 11 badged mints with active hooks, e.g. `5oCpEpFo…` (hook `Dercf2y5…`, 33 pools)                                                                                                                                                                                                                                                          | yes: `swap_v2` `RemainingAccountsInfo` slices `TransferHookA/B` (orca-so/whirlpools@408c945 `util/v2/remaining_accounts_utils.rs`) |
| Meteora DLMM       | with a token badge; 15 badged mints with hooks, e.g. `5Kd9TCEP…` (hook `BFy4nC9A…`, 13 pools)                                                                                                                                                                                                                                                                  | yes: `swap2` `remaining_accounts_info` slices `TransferHookX/Y` (dlmm-sdk@576919e `idls/dlmm.json`)                                |
| Meteora DAMM v2    | only inert hooks without a badge; none found in pools                                                                                                                                                                                                                                                                                                          | no remaining accounts in swap                                                                                                      |
| Meteora DAMM v1    | none found                                                                                                                                                                                                                                                                                                                                                     | no hook accounts in the instruction (from the SDK interface)                                                                       |
| PumpSwap           | inert hooks only. Pump admits a Token-2022 quote mint only with the xStock extension set, whose transfer hook has no program (pump-public-docs@8109141 `idl/pump.json` `create_v2`). The `quote-control` PDA `6z6GDdfb…` lists 183 mints at slot 451604733: 86 carry a hook extension, all with no program, e.g. SPYx `XsoCS1Tf…` (32 PumpSwap pools as quote) | no hook accounts in `buy`/`sell` (IDL), and none needed while the program is unset                                                 |

An inert hook still has an authority (on the xStocks, the issuer's) that can set a program later; the mint is in the market's closure, so `quoter` would see the change and refuse from that slot on. The same mints are pausable; a paused mint is refused like the other transfer states in [dexes.md](dexes.md) → Quotes.

OKX's open-source router passes no hook accounts either (`adapters/whirlpool.rs` sends `remaining_accounts_info = None`).

Routing hook mints therefore belongs to the Whirlpool and DLMM adapters: the market subscribes each hook mint's validation PDA and the accounts its seeds read; `quoter` resolves the extras per hop and puts them in the window; the router's wire already carries `hook_a` and `hook_b` counts per hop for them; the adapter writes the `RemainingAccountsInfo` slices. Each hook's extras (9 to 15 on the Orca mints seen) come out of the 64-account budget, a writable extra must be writable in the transaction, and the hook programs to trust should be an allowlist, starting from the venues' badge lists.

### How a cycle is told

The router calls a route a cycle when its source and destination are the same **account**, and then requires `min_out > in_amount`. A route from the user's WSOL ATA to another WSOL account the user owns is not a cycle to it. `tx` compares mints and refuses such a route with `UNPROFITABLE_CYCLE`, so nothing the server builds reaches the program that way.

OKX, whose router Pallas builds for, checks less (checked 2026-09-29):

- Its open-source router ([okxlabs/Web3-DEX-Router-Solana-V1](https://github.com/okxlabs/Web3-DEX-Router-Solana-V1) @ `677d3ec`, `programs/dex-solana/src/instructions/common_swap.rs`) measures the destination account's balance change against `min_return` and never compares mints. When source and destination are one account, that change is output minus input, so a losing cycle fails its `checked_sub`. The deployed program, `6m2CDdhRgxpH4WjvdzxAYbGxwdGUz5MziiL5jek2kBma`, last ran at slot 436855038 and its ProgramData is closed.
- Pallas builds for `proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u` ([okx/dex-solana-binary](https://github.com/okx/dex-solana-binary) `docs/api-swap-instruction.md`), whose source is not published. Its docs tell a cycle by mint in the API (`fromTokenAddress == toTokenAddress`), set the on-chain minimum to the quoted output less slippage, and say a cycle's quote may pay less than its input (`docs/cyclic-arbitrage.md`). Nothing on chain requires a profit.

So our program is already stricter than both, and the mint check lives where they keep it, off chain. A mint check in the program would cost two account reads per route. Whether to add one is still open.

## CI

The `replay` job runs `just replay-check` ([router.md](router.md) → Replay check): eight router fixtures rebuilt from the tree and executed on mainnet's bytecode. The large single-venue corpora (`just router-replay`, `router-orca-replay`, `router-dlmm-replay` and the other per-venue recipes) still run by hand only. The workflow runs only on pushes to `main` and on pull requests, so a pushed branch alone is not checked.

The replay job needs the release `just publish-oracle-programs` uploads for the current `programs.tsv`; after `just oracle` dumps changed bytecode, the fixtures are regenerated and a human publishes the new release, or the job fails at the download step.

`oracle router*` replays at a pinned 5,080 lamports per byte, mainnet's rate since SIMD-0437's second step. A later step changes the lamports of every account a replay creates: the constant in `oracle/src/router.rs` and all router fixtures move together, in one change.
