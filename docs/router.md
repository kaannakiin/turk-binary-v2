# Router program

`onchain/` is the on-chain half of `/swap-instructions`: a pinocchio program that runs a route the bot already quoted, hop by hop, in one instruction. It computes no prices. It checks that every hop moved the money it claims to have moved and that the route paid at least `min_out`.

## Flow wire format

The versioned flow instruction uses discriminator `4` and wire version `1`. It carries up to 16 ordered swap steps over up to 18 logical token slots. Slot `0` is the user's exact-in source and slot `1` is the final output; the remaining slots are intermediate virtual balances tied to the user's token accounts. Each step names a source and destination slot, a venue `Hop`, and a `u64` allocation fraction of the source's currently available balance. Arithmetic uses checked `u128` multiplication and floors the result; a step whose numerator equals its denominator consumes the remaining source balance.

`just router-flow-replay` regenerates the four-operation SOL→USDC and
SOL→IMG→USDC split, then merges both USDC credits into a USDC→DAILY swap.
IMG and DAILY are Token-2022 mints; DAILY charges an output transfer fee in the
captured epoch. The HTTP builder supplied the plan; the oracle separately sent
each venue instruction on one evolving LiteSVM bank, using actual balance
differences for later steps. Direct venue execution, router instruction and
unsigned v1 transaction each paid `10143850221706` units for `100000000`
lamports input. The captured result is
`crates/tx/src/tests/fixtures/router_flow_replay.json` (corpus SHA-256
`042e8b5e2410cec6201fc44a87a10f5adba07cf935f9dd9726b20d1850bbcdef`,
router binary SHA-256
`38c020a864b3d9b5fa77edd77317b717999c5c949f0a501043bb2f9cab2425d3`).
The same replay also executes two successive swaps through one CPMM pool,
spending two half allocations of `100000000` lamports. The second quote uses
the private state after the first operation. Direct program execution, router
instruction and v1 transaction each paid `8548616` units. These captures
verify the two listed flows; repeated CLMM, Orca, AMM v4 and DLMM transitions
remain gated pending independent state verification.
It also repeats the four-operation plan with `1000000` units already in the
user's USDC intermediate account. The final payout remains identical, proving
the existing balance is not credited as this plan's output. In all three cases,
raising the aggregate minimum output by one above the direct venue payout
fails atomically: the recorded pool accounts and user token accounts are
unchanged. The fee payer's transaction fee is outside that balance comparison.

Flow accounts begin with the existing `[user, source, destination, config]` prefix, followed by one account reference per slot and then the venue windows in step order. Duplicate account references are compiled once. The program measures the actual source and destination balance deltas after every CPI, so pre-existing destination balances cannot satisfy a step. Every flow operation must consume its full ExactIn allocation; all nonfinal credits must be zero at completion. Partial venue consumption or stranded input/intermediate credit fails atomically. It rejects self-slot operations, cycles in the logical dependency graph, an unproduced final slot, unconsumed intermediate producers, a source's last outgoing allocation that leaves a remainder, and routes that spend more than the root exact-in amount. `min_out` applies to the final slot; a cyclic route must also increase the root mint balance.

The legacy linear route remains discriminator `0` and keeps its four-hop limit. Flow has its own step and slot limits, account and compute budgets, and atomic failure behavior. Quote and transaction builders must emit the same ordered steps, slot references, and venue windows; the router does not re-quote or infer missing operations on chain.

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

**Status:** Raydium AMM v4 (kind 0), CLMM (kind 1), CPMM (kind 2), Orca Whirlpool (kind 3), and Meteora DLMM (kind 4) adapters. `just router-replay` runs a chosen venue's program replay corpus through the router, built by `/swap-instructions`, on the same accounts, Clock and mainnet bytecode: as `/swap-instructions`' instructions and as `/swap`'s unsigned v1 transaction signed by the LiteSVM test user. The CPMM corpus has 126 exact payouts (`crates/tx/src/tests/fixtures/router_replay.json`); the AMM v4 corpus has 162 (`crates/tx/src/tests/fixtures/router_replay_amm_v4.json`); the CLMM corpus has 129 (`crates/tx/src/tests/fixtures/router_replay_clmm.json`). Largest measured router executions use 32,803 CU for AMM v4 and 1,355,658 CU for CLMM. The Orca corpus has 129 positive exact payouts; 12 program-successful zero-output swaps are refused by the trading API, and 33 program failures remain failures. DLMM has 89 positive exact payouts through both API forms, with 713,797 CU as its measured v1 peak. No replay command signs or sends to mainnet.

The same command runs the scenarios the replay leaves out (`oracle router-scenarios`, results in `crates/tx/src/tests/fixtures/router_scenarios.json`, asserted by `tx`'s `scenarios` tests). They run over `scenario_pools.json.gz`, eight CPMM pools captured at one slot by `scripts/capture_cpmm_pools.py`, and each expected payout is what the pools paid swapped on their own:

`oracle router-matrix` also replays three-token routes on the slot-451598550 AMM v4/CPMM capture. Two selected pools force each order: AMM v4→CPMM, CPMM→AMM v4, and AMM v4→AMM v4. A simple `/quote` finds each path; both swap endpoints build that path; the unsigned `/swap` v1 bytes are signed and sent in LiteSVM. All three net payouts equal sequential direct venue execution (`crates/tx/src/tests/fixtures/router_amm_v4_matrix.json`). The same fixture records a naturally profitable AMM v4→CPMM cycle and an AMM v4→AMM v4 cycle made profitable by changing only the second SOL vault in LiteSVM. Direct program swaps find the smallest profitable vault balance; one unit less pays only the input. Both cycles accept the exact payout threshold and reject one unit above it without changing any user token or venue balances. The fee payer still pays the transaction fee on a failed transaction.

A separate slot-451601061 capture tests AMM v4 USDC→SOL followed by CPMM SOL→SOLADAO, a Token-2022 mint with a 25% output transfer fee at epoch 1045. Direct CPMM execution debits 30,640,141,758 units from its output vault and credits the user 22,980,106,318 units; `/swap` pays the same net amount (`crates/tx/src/tests/fixtures/router_amm_v4_token22.json`). AMM v4 itself rejects Token-2022 mints and vaults. The market snapshot omits CPMM observation accounts; `oracle router-matrix` fetches them once, records their bytes in the replay fixture and reuses them for deterministic repeats. Direct and router executions use the same account state. Run `just router-matrix-replay` to repeat the matrix.

The slot-451631965 capture (`crates/tx/src/tests/fixtures/clmm_cross_dex.json`) holds CLMM SOL/USDC, CPMM USDC/USDT, and AMM v4 USDC/USDT pools, tick arrays, observations, and Clock from one RPC bank. Four API-generated v1 routes cover CLMM→CPMM, CPMM→CLMM, CLMM→AMM v4, and AMM v4→CLMM. All four net payouts equal sequential direct venue swaps in LiteSVM. Each route accepts the exact payout threshold and rejects one unit above it atomically. Raising either hop's venue threshold by one also fails atomically in all four orders. Missing, wrong, or reversed tick arrays fail without changing balances; one-CU and one-byte loaded-data limits do too (`crates/tx/src/tests/fixtures/router_clmm_cross.json`). Run `just router-clmm-cross-replay` to regenerate the matrix.

The slot-451648543 Orca/Raydium capture (`crates/tx/src/tests/fixtures/orca_cross_dex.json`) replays all six Orca ↔ AMM v4/CPMM/CLMM directions. CLMM → Orca was refused as `TOO_MUCH_COMPUTE` while each hop was budgeted by the arrays it walks (setup 150,000 + CLMM 500,000 + Orca two-array 840,000); budgeted by its walk it fits, and it pays what the venues pay one by one. `recorded_orca_cross_dex_routes_build_and_quote_what_the_programs_paid_within_the_compute_budget` rebuilds all six from the snapshot with the current builder on every test run. Each two-hop v1 payout matches sequential direct program execution. Route and per-hop thresholds accept the exact payout and fail one unit above it without changing user token or venue balances; one-CU and one-byte loaded-data limits also fail (`router_orca_cross.json`). Whirlpool takes its named and supplemental tick arrays in any order and picks the ones the pool's current tick needs, so the router checks only that each belongs to the pool: the named arrays reversed pay the direct payout, and after another trader's swap moves the route's first tick array by one, the route lands through its supplemental arrays and pays what the moved venues pay directly; the same route without them fails in Whirlpool with `InvalidTickArraySequence`. `just router-orca-cross-replay` regenerates this result. `just router-orca-replay` reruns the 129 paid single-hop Whirlpool cases. The captured AI66→SOL→USDC→AI66 three-hop cycle quotes below its input and is rejected as `UNPROFITABLE_CYCLE` when the full snapshot is supplied to the planner. Active Token-2022 transfer hooks are rejected. The separate slot-451651937 fee snapshot (`orca_fee_pools.json`) replays seven mixed SPL/Token-2022 swaps with real input or output transfer fees; router payouts match direct Whirlpool execution (`router_orca_fee.json`, `just router-orca-fee-replay`). One reverse direction has no paid route at that slot. The fee schedule switch is replayed at epochs 847 and 848: the same input pays 8,286,606 then 7,872,275 units, matching the direct program in each epoch. The slot-451655001 Token-2022/Token-2022 pool replays both directions and exact thresholds against direct program execution (`orca_token22_pair.json`, `router_orca_pair.json`, `just router-orca-pair-replay`).

DLMM `swap2` takes the exact bin arrays consumed by the quote in their walk order. Its 16 fixed accounts include optional bitmap extension and host fee slots; absent options use the DLMM program ID. The adapter checks linked pool accounts, token programs and bin array order. The 89 paid single-hop cases match direct Meteora payouts in both API forms (`router_dlmm_replay.json`). A real Token-2022 transfer-fee pool at slot 451672871 matches direct net payouts in both directions; wrong, missing and readonly window accounts fail atomically (`router_dlmm_fee.json`). A two-array corpus swap also rejects reversed arrays and pays the direct amount (`router_dlmm_two_array.json`). An overflow-bitmap pool at slot 451674051 pays the direct amount in both directions (`router_dlmm_extension.json`). The adapter takes an oracle as long as its header's sample count makes it (32 + 32 × `length` bytes): the slot-452267679 SOL/USDC pool `BGm1ta…`, whose oracle was grown to 206 samples (6,624 bytes), pays the direct amount in both directions and refuses one unit more (`router_dlmm_grown_oracle.json`, `just router-dlmm-grown-oracle-replay`); the router before this check took only the 100-sample 3,232 bytes and refused it as `BadWindow`. At slot 451671159, the DLMM→CLMM USDC→AI66→SOL v1 route pays 92,896 lamports, the direct two-venue amount; raising either hop's minimum by one aborts the route (`router_dlmm_cross.json`). `tx` budgets the deployed DLMM ProgramData at 2,229,821 bytes (mainnet RPC slot 451667468). Each DLMM hop is budgeted by the bins its quote crosses ([architecture.md](architecture.md) → `computeUnitLimit`); the builder rejects a window of more than three bin arrays, the most any measured swap took (a fourth contiguous array is at least 211 bins, past what 1.4 million CU pays for), and refuses a route above 1.4 million CU.

The slot-451266433 Orca/CLMM capture (`orca_cycle.json`) has a two-hop SOL→USDC→SOL cycle whose direct venue payouts are 118,357 USDC units and 1,000,357 SOL units from 1,000,000 input units. The router v1 replay accepts the exact token payout and atomically rejects one unit more, as well as both raised hop thresholds (`router_orca_cycle.json`; `just router-orca-cycle-replay`). Its 357-lamport token gain is below the 5,000-lamport transaction fee, so this fixture verifies route mechanics and is not an economically profitable trade. The API's cycle gate only compares token amounts; a live strategy must also enforce its configured profit threshold after fees. The replay fetches the CLMM observation account from public RPC because this older snapshot predates that account's closure capture.

- SOL → USDC → NEAR as `/swap` builds it, with 500 USDC already in the intermediate account: the second hop spends only what the first paid, the 500 USDC stay, and the setup creates the NEAR account within the transaction's budgets.
- SOL wrapped with and without an existing WSOL account, and SOL unwrapped as the output: the lamports come out exactly as the amounts, rent and fee say. Closing a funded WSOL account also unwraps what it held before.
- Token-2022: SOL → DHC into a Token-2022 account the setup creates; SOL → USDC → DAILY (3% transfer fee), where the user receives what the fee leaves; DAILY → USDC, where the user is debited the full input; SOL → IMG → USDC, where the 5% fee mint sits between the hops and the second hop spends exactly what arrived; SOL → SOLADAO at epochs 1043 and 1045, either side of its fee change from 30% to 25%; WIWI → MU, a mint with a transfer hook extension but no hook program.
- Every threshold `/swap` built is the pools' own payout less the default 50 bps, transfer fee included: the quote was exact.
- A threshold one unit above the payout fails with `SlippageExceeded` and moves no token; at the payout the route passes.
- A venue that misbehaves: `onchain/programs/short-venue`, a test double never deployed, loaded at CPMM's address and moving what the scenario sets. Taking one unit below 95% of the offer or one unit above it fails with `ActualInOutOfBand`; exactly 95% passes and leaves the rest with the user. Paying nothing fails with `ZeroHopOutput`, taking output back from the user with `BalanceRegression`.
- The `/swap-instructions` route with one byte or one account wrong, each refused by its own check: a zero `min_out` or input, wire version 1, zero or five hops, an unknown hop kind, a destination the user does not own, a source that is not a token account, a cycle without profit, a source or destination the hops do not start or end on, a window one account short or long, another program in a window's venue slot, and a config account owned by the router but not at its PDA.
- The admin instructions with the router deployed under its own upgrade authority: `initialize` refused to a stranger, with another ProgramData or a zero admin, accepted after someone sent lamports to the config first, refused a second time; routes refused before `initialize` and while paused; `set_paused` and `set_admin` refused to anyone but the admin, including the upgrade authority, and to an unsigned admin.

What neither covers yet is in [open-work.md](open-work.md).

**Replay check:** the replays above write fixtures that `tx`'s tests read, so a later change to the builder or the router is caught only if someone re-runs them. `just replay-check` (`scripts/replay_check.py`, CI's `replay` job) re-runs eight of them from the current tree on every pull request: `router_scenarios.json` (funded intermediate, WSOL, Token-2022 fees, thresholds, misbehaving venue, admin), `router_flow_replay.json` (split → merge), `router_amm_v4_matrix.json` (two-hop routes and cycles), `router_amm_v4_token22.json` (25% output fee), `router_clmm_cross.json` (missing, wrong and reversed tick arrays), `router_orca_cross.json` (moved tick array, supplemental arrays) `router_dlmm_two_array.json` (wrong, missing and reversed bin arrays) and `router_dlmm_grown_oracle.json` (an oracle grown to 206 samples). Each goes through the `server` plan test, the router and short venue built with platform-tools `v1.53`, and `oracle` with `ORACLE_OFFLINE=1`, which refuses any account fetch; the committed fixture is only read, and each result is a new file the oracle must write (`router-matrix` reads its cached observation accounts from the committed fixture, the optional `OBSERVATIONS_FROM` argument). The output must equal the committed fixture in every field except compute units and the router's ELF hash, which it reports; errors compare by kind (`InstructionError(1, Custom(6013))`), not by log text. Program bytecode comes from the release `just publish-oracle-programs` uploads and must match `programs.tsv`. Each run writes into a new `target/replay-check/run-*` directory and deletes nothing; its `replay-report.json` records the commit, rustc, cargo-build-sbf and platform-tools, LiteSVM, the SDK fork commits, every program's deploy slot and hash, the router's hash and every input's sha256. A change that alters a replay regenerates that fixture with its `just router-*-replay` recipe in the same change. `scripts/test_replay_check.py` (run first in the same job) pins the checker itself: a changed payout, rollback, error code or case count is a difference, log text and compute units are not, an oracle that exits 0 without writing a result fails, and `--out`'s existing files stay.

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
| 3    | Orca Whirlpool | `16 + tail`: Whirlpool program, `swap_v2`'s 15 accounts; `tail` is 0–2 optional supplemental tick arrays; no hook accounts             |
| 4    | Meteora DLMM   | `17 + tail`: DLMM program, `swap2`'s 16 fixed accounts, then `tail` consumed bin arrays in quote order; no hook accounts               |

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
