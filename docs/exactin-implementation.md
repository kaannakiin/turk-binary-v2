# ExactIn routing implementation

Approved scope: independent HTTP contract; exact-in only; normal swaps support
split/merge; cyclic arbitrage remains one unsplit cycle. No discovery endpoint,
Hub dependency, new venue integration, signing, deployment, or transaction sending.
Existing operator changes in AGENTS.md, .codex/, the orchestration skill and
docs/codex.md are outside this change.

## Contract

- Requests use fromTokenAddress, toTokenAddress, amount, slippagePercent,
  userWalletAddress, dexIds, excludedDexIds, allowedPools, directRoute,
  singleRouteOnly, singlePoolPerHop, enableCyclicArbitrage, uniqueDexIds,
  enableUniqueDex and maxHops. No old-name aliases.
- Decimal slippage defaults to 0.5%, accepts at most two decimal places and is
  converted exactly to basis points. Program addresses identify DEXes.
- Missing/null allowedPools means no filter; an empty array allows nothing.
- directRoute means exactly one pool and conflicts with cyclic arbitrage.
  singleRouteOnly forbids branching. singlePoolPerHop forbids different pools
  for the same directed mint pair. Cycles apply unique DEXes across the loop.
- Local configured unique DEX defaults are empty. A nonempty request replaces
  them. Invalid IDs are rejected even when uniqueness is disabled.
- Quote plans describe ordered operations, dependencies and allocations.
  Existing endpoints stay; old request field names are rejected.

## Delivery and validation

1. Baseline: fixed snapshot hash, source/SDK revisions, machine and toolchain;
   separate route, quote, build and HTTP measurements.
2. HTTP normalization and path constraints, with focused contract tests.
3. Stateful quote/execution model: isolated candidate state, correctly modeled
   repeated pools/shared writes, split/merge and checked amount conservation.
4. Versioned router plan, transaction assembly and independent LiteSVM replay.
5. Budgeted split search and layered search comparison against DFS. Preserve
   best valid incumbent and disclose pruning/exhaustion, including deadlines.
6. Profile-driven allocation changes only; SDK fork commits/pins only after
   independent replay and before/after measurement.
7. End-to-end validation, lint/CI, quality and performance report, small commits.

Every step uses implement → focused correctness checks → comparative measurement
→ review → commit. Financial expected values come from independent program
simulation/SDK fixtures. On-chain changes require program_autofixer. Critical
source conflicts block the affected change. Unsupported state transitions must
never silently produce a quote. Five currently routable transaction venues stay
in scope; all eight quote venues remain covered. All SDK types stay in quoter.

## Progress

Transaction format rechecked 2026-09-29 against
[SIMD-0385 at 4b643ca](https://github.com/solana-foundation/solana-improvement-documents/blob/4b643ca8746742183a469681765e694b385bb315/proposals/0385-transaction-v1.md)
and the official [transaction pipeline](https://solana.com/docs/core/transactions/transaction-pipeline).
The specification confirms prefix 129, inline unique addresses, maximum 64
addresses and no lookup tables. The pipeline documents 4096 bytes for v1 and
zero defaults for omitted config limits. The proposal still labels itself
Review; deployment evidence is the existing recorded RPC verification in
AGENTS.md, not inferred from proposal status. Assembly keeps these existing
limits; no new network activation claim is made.

- Baseline source: b8f2eaf. Capture and timings: [baseline](exactin-baseline.md).
- HTTP normalization, filter intersection, DAG response/roundtrip, amount and
  depth validation implemented. Search cancellation follows request cancellation
  and its original deadline, including queue time.
- Flow allocations merge equal edges before quoting. Search retains its best
  admitted candidate; transaction construction checks account/data/compute/byte
  budgets before a swap candidate can replace that incumbent.
- Versioned router flow and transaction assembly implemented; independent review
  requested fixes for cyclic roundtrip validation, compute overflow admission and
  unfinished allocation graphs. Regression coverage accompanies the fixes.
- CPMM private sequential transitions implemented and verified on one same-pool
  two-operation LiteSVM replay. A separate four-operation split/merge with a
  Token-2022 fee branch also matched direct venue swaps, router execution and
  unsigned v1 transaction. A prefunded intermediate account paid the same final
  amount; raised minimum outputs failed without changing pool/user token accounts
  in all three replay cases. A short venue that consumed exactly 95% of a flow
  allocation failed with `ActualInOutOfBand` and left captured state unchanged
  (`just router-flow-replay`). Other repeated-pool transitions
  and distinct pools sharing writable state remain explicitly unsupported until
  independently verified. This is an outstanding scope item, not a completed
  shared-state implementation.
- Layered search remains experimental. [Search measurements](exactin-performance.md)
  do not justify replacing DFS; split quality gains cost additional CPU.
- [HTTP measurements](exactin-http-performance.md) report allocation counts and
  sampled resident memory. No SDK fork change or unmeasured speedup claimed.
- Reproducible three-case real-program replay and one partial-input refusal are
  in the tree. Wider shared-state replay remains pending.

## Integration checks (in progress)

`just lint` and cargo-deny completed during the CI run. Full CI found three
pre-existing fixture failures, reproduced unchanged in detached baseline
`b8f2eaf` (nextest run `82968560-96e0-4f8a-aa76-fa84f85f075a`):

- `dex::tests::arrays::every_required_dependency_of_the_complex_pools_exists_with_an_accepted_owner`:
  captured Raydium CLMM observation account missing.
- `quoter::tests::svm::raydium_clmm_pays_what_the_deployed_program_pays`:
  observation missing for pool `2JtkunkYCRbe5YZuGU6kLFmNwN22Ba1pCicHoqW5Eqja`.
- `market::tests::a_new_bitmap_bit_subscribes_its_tick_array_and_seeds_it`:
  initial array wait times out; separately reproduced in baseline release run
  `ad0a7f35-5704-458f-98e4-b601806b69ee` using its isolated target directory.

The final `just ci` run on the integrated ExactIn change completed lint and
cargo-deny, then ran 365 tests: 362 passed, the same three baseline tests
failed, and 16 were skipped. `cargo test --workspace --doc` passed when run
separately. `just test-onchain` passed 30/30; `just lint-onchain`, SBF build,
and the independent flow replay passed.

Fixture bytes and assertions were not fabricated, removed, or relaxed. These
failures prevent claiming a green full CI independently of the flow work.
