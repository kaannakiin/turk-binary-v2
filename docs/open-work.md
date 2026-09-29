# Open work

What is known to be missing, with why it matters. An item leaves this page in the change that does it.

## A live feed of what a transaction loads

`tx` sizes a v1 transaction's `loadedAccountsDataSizeLimit` from a table in `crates/tx/src/budget.rs`: each program's programdata size measured on mainnet once (slot 451401804), half again for growth, plus a flat 10 KiB per other account. It is safe (too small only fails the transaction, it loses nothing) but it goes stale: a program upgrade can outgrow it, and a venue added without a row is refused (`UNSUPPORTED_VENUE`).

The budget should come from a structure fed continuously instead, as Pallas serves it from its own endpoint:

- subscribe the programdata account of every program a route can invoke or pass (venue programs, both token programs, the ATA program, the router) through the gRPC hub, and keep their sizes current;
- take every other account's data length from the market's own views, which already hold them;
- compute the limit exactly per SIMD-0186 (data length plus 64 bytes per account, programdata of LoaderV3 programs), with headroom for the accounts the setup creates, rounded up to 32 KiB pages (SIMD-0553).

The compute unit limit has the same shape: per-venue budgets from `just router-replay` today, better measured per route (simulation, or a replay-fed table per venue and hop shape).

## Router behaviour not yet proven by execution

`just router-replay` runs single-hop CPMM swaps with the user's accounts already created, SOL neither wrapped nor unwrapped, and the router's config written directly as initialized and unpaused. Before a mainnet deploy, each of these needs a run on the real program:

| Test                                                                          | What it proves                                                                                                                              |
| ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Two hops with a balance already in the intermediate account                   | Only what the route produced is spent onward; the rest stays with the user.                                                                 |
| A route that pays less than its threshold                                     | The transaction fails and every swap in it is rolled back.                                                                                  |
| SOL wrapped and unwrapped, with and without a WSOL account                    | Setup and cleanup leave the balances stated; closing the WSOL account also unwraps SOL the user already held there, which the API must say. |
| Setup that creates the output account                                         | The measured compute and loaded data still fit the budgets.                                                                                 |
| `initialize` by the upgrade authority and by anyone else; pause; admin change | The admin checks hold in program execution, not only in review.                                                                             |

## CI

The `onchain` job runs host tests only. `cargo build-sbf` and `just router-replay` do not run in CI, and the workflow runs only on pushes to `main` and on pull requests, so a pushed branch alone is not checked.
