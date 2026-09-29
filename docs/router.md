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

**Status:** one venue adapter, Raydium CPMM (kind 2). `just router-replay` runs every swap of the CPMM program replay corpus through the router, built by `/swap-instructions`, on the same accounts, Clock and mainnet bytecode: twice, as `/swap-instructions`' instructions and as `/swap`'s unsigned v1 transaction, only signed. All 126 pay exactly what the venue paid alone both ways, in at most 42,293 compute units with their setup (`crates/tx/src/tests/fixtures/router_replay.json`). What the replay does not cover yet is in [open-work.md](open-work.md).

**Deploy:** give the program at most 256 KiB of space (`--max-len`); `tx` budgets the router's loaded data at that size. Not deployed; nothing here has run on mainnet.

## How a route runs

1. The user signs; both user token accounts must be token accounts the user owns.
2. The config PDA is loaded (owner, discriminator, version, stored bump) and must not be paused.
3. `in_amount` and `min_out` are nonzero. When the source and destination accounts are the same (a cycle), `min_out` must exceed `in_amount`.
4. For each hop, the program derives the window length from the hop's kind and its `hook_a`, `hook_b`, `tail` counts, takes that many accounts, and lets the kind's adapter build the CPI. Slot 0 of a window is the venue program and must equal the adapter's program ID.
5. The hop's input account must be the previous hop's output account. After the CPI the input account must still be open, the venue must have pulled between 95% and 100% of what it was offered, and the output account must have grown. That growth is the next hop's input.
6. No accounts may be left over, the last output must be the destination account, and the last growth must be at least `min_out`.

## Instructions

Every instruction starts with a one-byte tag. Integers are little-endian.

| Tag | Instruction  | Data after the tag                                                          | Accounts                                                                                |
| --- | ------------ | --------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| 0   | `route`      | `version u8 = 1`, `in_amount u64`, `min_out u64`, `hop_count u8`, then hops | user (signer), source (w), destination (w), config, then each hop's window              |
| 1   | `initialize` | `admin [u8; 32]`                                                            | config (w), payer (signer, w), upgrade authority (signer), system program, program data |
| 2   | `set_paused` | `paused u8` (0 or 1)                                                        | config (w), admin (signer)                                                              |
| 3   | `set_admin`  | `new_admin [u8; 32]`                                                        | config (w), admin (signer)                                                              |

A hop is 4 bytes: `kind u8`, `hook_a u8`, `hook_b u8`, `tail u8`. `hook_a` and `hook_b` are the Token-2022 transfer-hook account counts of the pool's two mints; `tail` is a venue's variable account count (for example extra tick arrays). A `route` holds 1 to 4 hops and is exactly `19 + 4 × hop_count` bytes; any other length is refused.

Kinds follow `domain::DexKind`'s declaration order; a number is accepted only once its adapter exists:

| Kind | Venue        | Window                                                                               |
| ---- | ------------ | ------------------------------------------------------------------------------------ |
| 2    | Raydium CPMM | 14: the CPMM program, then `swap_base_input`'s 13 accounts; no hook or tail accounts |

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
