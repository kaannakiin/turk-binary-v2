# Router program

`onchain/` is the on-chain half of `/swap-instructions`: a pinocchio program that runs a route the bot already quoted, hop by hop, in one instruction. It computes no prices. It checks that every hop moved the money it claims to have moved and that the route paid at least `min_out`.

It is its own Cargo workspace, like `oracle/`: `cargo build-sbf` compiles it with platform-tools' rustc, and pinocchio's stack stays out of the bot's dependency graph.

| Crate                | Holds                                                                    |
| -------------------- | ------------------------------------------------------------------------ |
| `crates/router-wire` | Instruction and account codecs. No dependencies; clients encode with it. |
| `crates/router-core` | Route checks, token-account reads, venue adapters. No dependencies.      |
| `programs/router`    | The pinocchio shell: entrypoint, account reads, CPI.                     |

```sh
just lint-onchain    # fmt --check + clippy -D warnings
just build-onchain   # cargo build-sbf → onchain/target/deploy/router.so
just test-onchain    # nextest over the onchain workspace
```

**Program ID:** `TURKAGEDZ6JgA9eSQydhARcWSc2hps5T8v1ouhi84L3`, not deployed.

**Status:** Raydium AMM v4 (kind 0), CLMM (kind 1), and CPMM (kind 2) adapters. `just router-replay` runs a chosen venue's program replay corpus through the router, built by `/swap-instructions`, on the same accounts, Clock and mainnet bytecode: as `/swap-instructions`' instructions and as `/swap`'s unsigned v1 transaction signed by the LiteSVM test user. The CPMM corpus has 126 exact payouts (`crates/tx/src/tests/fixtures/router_replay.json`); the AMM v4 corpus has 162 (`crates/tx/src/tests/fixtures/router_replay_amm_v4.json`); the CLMM corpus has 129 (`crates/tx/src/tests/fixtures/router_replay_clmm.json`). Largest measured router executions use 32,803 CU for AMM v4 and 1,355,658 CU for CLMM. Neither path signs or sends to mainnet.

The same command runs the scenarios the replay leaves out (`oracle router-scenarios`, results in `crates/tx/src/tests/fixtures/router_scenarios.json`, asserted by `tx`'s `scenarios` tests). They run over `scenario_pools.json.gz`, eight CPMM pools captured at one slot by `scripts/capture_cpmm_pools.py`, and each expected payout is what the pools paid swapped on their own:

`oracle router-matrix` also replays three-token routes on the slot-451598550 AMM v4/CPMM capture. Two selected pools force each order: AMM v4→CPMM, CPMM→AMM v4, and AMM v4→AMM v4. A simple `/quote` finds each path; both swap endpoints build that path; the unsigned `/swap` v1 bytes are signed and sent in LiteSVM. All three net payouts equal sequential direct venue execution (`crates/tx/src/tests/fixtures/router_amm_v4_matrix.json`). The same fixture records a naturally profitable AMM v4→CPMM cycle and an AMM v4→AMM v4 cycle made profitable by changing only the second SOL vault in LiteSVM. Direct program swaps find the smallest profitable vault balance; one unit less pays only the input. Both cycles accept the exact payout threshold and reject one unit above it without changing any user token or venue balances. The fee payer still pays the transaction fee on a failed transaction.

A separate slot-451601061 capture tests AMM v4 USDC→SOL followed by CPMM SOL→SOLADAO, a Token-2022 mint with a 25% output transfer fee at epoch 1045. Direct CPMM execution debits 30,640,141,758 units from its output vault and credits the user 22,980,106,318 units; `/swap` pays the same net amount (`crates/tx/src/tests/fixtures/router_amm_v4_token22.json`). AMM v4 itself rejects Token-2022 mints and vaults. The market snapshot omits CPMM observation accounts; `oracle router-matrix` fetches them once, records their bytes in the replay fixture and reuses them for deterministic repeats. Direct and router executions use the same account state. Run `just router-matrix-replay` to repeat the matrix.

The slot-451631965 capture (`crates/tx/src/tests/fixtures/clmm_cross_dex.json`) holds CLMM SOL/USDC, CPMM USDC/USDT, and AMM v4 USDC/USDT pools, tick arrays, observations, and Clock from one RPC bank. Four API-generated v1 routes cover CLMM→CPMM, CPMM→CLMM, CLMM→AMM v4, and AMM v4→CLMM. All four net payouts equal sequential direct venue swaps in LiteSVM. Each route accepts the exact payout threshold and rejects one unit above it atomically. Raising either hop's venue threshold by one also fails atomically in all four orders. Missing, wrong, or reversed tick arrays fail without changing balances; one-CU and one-byte loaded-data limits do too (`crates/tx/src/tests/fixtures/router_clmm_cross.json`). Run `just router-clmm-cross-replay` to regenerate the matrix.

- SOL → USDC → NEAR as `/swap` builds it, with 500 USDC already in the intermediate account: the second hop spends only what the first paid, the 500 USDC stay, and the setup creates the NEAR account within the transaction's budgets.
- SOL wrapped with and without an existing WSOL account, and SOL unwrapped as the output: the lamports come out exactly as the amounts, rent and fee say. Closing a funded WSOL account also unwraps what it held before.
- Token-2022: SOL → DHC into a Token-2022 account the setup creates; SOL → USDC → DAILY (3% transfer fee), where the user receives what the fee leaves; DAILY → USDC, where the user is debited the full input; SOL → IMG → USDC, where the 5% fee mint sits between the hops and the second hop spends exactly what arrived; SOL → SOLADAO at epochs 1043 and 1045, either side of its fee change from 30% to 25%; WIWI → MU, a mint with a transfer hook extension but no hook program.
- Every threshold `/swap` built is the pools' own payout less the default 50 bps, transfer fee included: the quote was exact.
- A threshold one unit above the payout fails with `SlippageExceeded` and moves no token; at the payout the route passes.
- A venue that misbehaves: `onchain/programs/short-venue`, a test double never deployed, loaded at CPMM's address and moving what the scenario sets. Taking one unit below 95% of the offer or one unit above it fails with `ActualInOutOfBand`; exactly 95% passes and leaves the rest with the user. Paying nothing fails with `ZeroHopOutput`, taking output back from the user with `BalanceRegression`.
- The `/swap-instructions` route with one byte or one account wrong, each refused by its own check: a zero `min_out` or input, wire version 1, zero or five hops, an unknown hop kind, a destination the user does not own, a source that is not a token account, a cycle without profit, a source or destination the hops do not start or end on, a window one account short or long, another program in a window's venue slot, and a config account owned by the router but not at its PDA.
- The admin instructions with the router deployed under its own upgrade authority: `initialize` refused to a stranger, with another ProgramData or a zero admin, accepted after someone sent lamports to the config first, refused a second time; routes refused before `initialize` and while paused; `set_paused` and `set_admin` refused to anyone but the admin, including the upgrade authority, and to an unsigned admin.

What neither covers yet is in [open-work.md](open-work.md).

**Deploy:** give the program at most 256 KiB of space (`--max-len`); `tx` budgets the router's loaded data at that size. Not deployed; nothing here has run on mainnet.

## How a route runs

1. The user signs; both user token accounts must be token accounts the user owns.
2. The config PDA is loaded (owner, discriminator, version, stored bump) and must not be paused.
3. `in_amount` and `min_out` are nonzero. When the source and destination accounts are the same (a cycle), `min_out` must exceed `in_amount`.
4. For each hop, the program derives the window length from the hop's kind and its `hook_a`, `hook_b`, `tail` counts, takes that many accounts, and lets the kind's adapter build the CPI with that hop's `min_out`. Slot 0 of a window is the venue program and must equal the adapter's program ID.
5. The hop's input account must be the previous hop's output account. After the CPI the input account must still be open, the venue must have pulled between 95% and 100% of what it was offered, and the output account must have grown. That growth is the next hop's input.
6. No accounts may be left over, the last output must be the destination account, and the last growth must be at least `min_out`.

## Instructions

Every instruction starts with a one-byte tag. Integers are little-endian.

| Tag | Instruction  | Data after the tag                                                          | Accounts                                                                                |
| --- | ------------ | --------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| 0   | `route`      | `version u8 = 2`, `in_amount u64`, `min_out u64`, `hop_count u8`, then hops | user (signer), source (w), destination (w), config, then each hop's window              |
| 1   | `initialize` | `admin [u8; 32]`                                                            | config (w), payer (signer, w), upgrade authority (signer), system program, program data |
| 2   | `set_paused` | `paused u8` (0 or 1)                                                        | config (w), admin (signer)                                                              |
| 3   | `set_admin`  | `new_admin [u8; 32]`                                                        | config (w), admin (signer)                                                              |

A hop is 12 bytes: `kind u8`, `hook_a u8`, `hook_b u8`, `tail u8`, `min_out u64`. The venue CPI enforces `min_out` for every hop; the router also checks the final credited output against the route `min_out`. `hook_a` and `hook_b` are the Token-2022 transfer-hook account counts. For CLMM, `tail`'s low seven bits count tick arrays and its high bit denotes a preceding bitmap extension. A `route` holds 1 to 4 hops and is exactly `19 + 12 × hop_count` bytes; any other length is refused.

The API computes each hop's `min_out` from its quoted net output and requested slippage; the last hop also meets the route minimum. Raydium CPMM compares its own `minimum_amount_out` against the output **after** Token-2022 transfer fees (`swap_base_input.rs` in the pinned fork), so the net threshold is passed directly.

Kinds follow `domain::DexKind`'s declaration order; a number is accepted only once its adapter exists:

| Kind | Venue          | Window                                                                                                                                 |
| ---- | -------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| 0    | Raydium AMM v4 | 9: the AMM program, then `SwapBaseInV2`'s 8 accounts; SPL Token only, no hook or tail accounts                                         |
| 1    | Raydium CLMM   | `14 + arrays + extension`: the CLMM program, `swap_v2`'s 13 accounts, optional bitmap extension, then tick arrays in bitmap walk order |
| 2    | Raydium CPMM   | 14: the CPMM program, then `swap_base_input`'s 13 accounts; no hook or tail accounts                                                   |

The window length is not sent: it follows from the kind, so a length that disagrees with the accounts cannot be expressed.

`initialize` creates the config paused. Only the program's upgrade authority can call it: the program data account must be the loader's PDA of `[program_id]`, owned by the upgradeable loader, and name the signer as its upgrade authority. Otherwise whoever called it first after the deploy would become the admin. A config PDA someone pre-funded is topped up, allocated and assigned instead of created.

## Config account

PDA of `[b"config"]`, 36 bytes, owned by the program:

| Offset | Field         | Value          |
| ------ | ------------- | -------------- |
| 0      | discriminator | `1`            |
| 1      | version       | `1`            |
| 2      | `admin`       | 32 bytes       |
| 34     | `paused`      | 0 or 1         |
| 35     | `bump`        | canonical bump |

## Errors

`Custom(code)`:

| Code | Error                    | Code | Error                        |
| ---- | ------------------------ | ---- | ---------------------------- |
| 6000 | `Paused`                 | 6009 | `HopContinuityViolation`     |
| 6001 | `BadArgs`                | 6010 | `ActualInOutOfBand`          |
| 6002 | `BadHopCount`            | 6011 | `ZeroHopOutput`              |
| 6003 | `UnsupportedWireVersion` | 6012 | `BalanceRegression`          |
| 6004 | `AtaOwnerMismatch`       | 6013 | `SlippageExceeded`           |
| 6005 | `WindowOutOfBounds`      | 6014 | `CircularRouteNotProfitable` |
| 6006 | `UnknownHopKind`         | 6015 | `NotAdmin`                   |
| 6007 | `BadWindow`              | 6016 | `ZeroAdmin`                  |
| 6008 | `NotATokenAccount`       | 6017 | `NotUpgradeAuthority`        |
|      |                          | 6018 | `BadProgramData`             |

Malformed instruction data other than a wrong `route` version or hop count is `InvalidInstructionData`.
