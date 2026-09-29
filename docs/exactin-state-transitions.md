# Sequential pool state: verification status

Equal directed edges merge before quoting. This avoids reusing stale liquidity
for a shared prefix or suffix. Separate operations on one pool require a private,
mutable state simulation. Different pools sharing writable state require account
level synchronization, not merely cloning each venue independently.

| Venue | Current implementation | Remaining verification/work |
| --- | --- | --- |
| Raydium CPMM | Private transition applies SDK fee accounting and net vault changes. | Two swaps through one pool matched direct LiteSVM program output and both router forms at the captured slot; broaden amounts/directions before claiming exhaustive coverage. |
| Raydium AMM v4 | Repeated operations refused. | Program processor updates both vault amounts and recent epoch; implement and replay both directions. |
| Raydium CLMM | Repeated operations refused. | Retain updated pool, crossed ticks, bitmap, observation and vaults from the existing mutable program call. Current quote discards clones and constructs a fresh observation. |
| Orca | Repeated operations refused. | Core quote returns scalar pool changes but not all crossed tick mutations; use verified mutable program swap manager and oracle updates. |
| Meteora DLMM | Repeated operations refused. | Existing SDK quote has no complete mutable bin/pair/oracle transition; program state differences need independent validation. |

Read-only source inspection on 2026-09-29 found the following concrete paths:

- AMM v4 pinned program `program/src/processor.rs`, swap input/output transfer and
  `recent_epoch` update; adapter `crates/quoter/src/raydium_amm_v4/mod.rs`.
- CLMM `programs/amm/src/instructions/swap.rs::swap_internal_with_key`,
  `states/pool.rs`, `instructions/swap_v2.rs`; observation is already part of the
  DEX dependency closure.
- Orca `programs/whirlpool/src/manager/swap_manager.rs`,
  `util/v2/swap_utils.rs`, `instructions/v2/swap.rs`. Core
  `rust-sdk/core/src/quote/swap.rs::SwapResult` alone is insufficient.
- DLMM `commons/src/quote.rs`: local pair/array clones are discarded and the bin
  quoting helper reads an immutable bin. IDL writable accounts identify affected
  accounts but do not specify their arithmetic transitions.

These findings guide future implementation; they do not satisfy the two-source
and independent replay gates by themselves. No unsupported transition is enabled.
The present search generates mint DAGs and does not discover repeated-pool paths;
CPMM sequential support is currently available only when repricing supplied plans.
General shared-write routing is **not complete**.
