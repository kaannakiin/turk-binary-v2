---
name: test-audit
description: "Load whenever writing, changing, reviewing, or deleting tests. Authoring gate for new tests plus an audit workflow for low-value, implementation-coupled, or duplicative tests and the test-only production seams they demand. Enforces that quote and layout tests take expected values from an independent source."
---

# Test Audit

Adapted for turk-binary-v2 from `openclaw/openclaw` `.agents/skills/test-audit`.

Two modes, one value bar. **Authoring mode** gates every new or changed test at
write time. **Audit mode** hunts for junk patterns in existing tests. Optimize
for confidence, not deletion count.

In this repo a test either catches a bug that loses money or it is maintenance
cost. The most expensive failure is a test that is green but wrong: a test that
checks ported math against its own output carries a wrong quote to mainnet.

## Authoring gate

Before adding any test, answer four questions. A missing answer means do not
add it yet.

1. What observable behavior, invariant, or independent contract does it protect?
2. What credible regression makes it fail?
3. Why does existing coverage not already catch that failure? Each contract has
   one owner test at the strongest boundary. A second test at another layer
   needs its own distinct risk the owner cannot reach (stream drop, reconnect,
   shard routing). Prefer adding a row to an existing table-driven case over a
   near-duplicate test.
4. Does it need a production seam that no production caller needs (widened
   `pub`, `#[cfg(test)]` export, flag, injection hook)? If yes, move the test to
   the real boundary instead.

Then check the test against every entry in **Junk patterns** below. A match
fails the gate unless the **Retention bar** names the contract it independently
guards. A test that would break under behavior-preserving
refactoring is asserting implementation, not behavior; rewrite it at the owning
boundary.

**Bug regression tests** must fail on the pre-fix code for the intended reason
and pass after the fix. A regression test that never demonstrably failed proves
the mock, not the fix. Include the pre-fix failure output in the handover.

## Source of expected values

Every test touching critical information (AGENTS.md → Verification protocol) takes
its expected value from a source **independent** of the code under test:

| Test kind                                      | Expected value comes from                                                                                                                      | Never from                                                 |
| ---------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| Swap quote, fee, tick/bin/sqrt-price math      | Program simulation output (LiteSVM/Surfpool with cloned mainnet accounts) or test vectors from the program's own repo, with a `// src:` record | Our port's output, a hand-derived formula, a "close" value |
| Account layout, offset, decode                 | Real mainnet bytes captured with `scripts/capture_accounts.py`; field values from RPC `jsonParsed`, the official SDK decoder, or an explorer   | Hand-written byte arrays, the decoder's own output         |
| Discriminator, PDA seed, program ID            | IDL/source plus a real account in the fixtures (owner, first 8 bytes, derived address matches a real pool)                                     | Comparing a constant against the same literal              |
| Pool filter (`AccountFilter`, memcmp/datasize) | Real fixture accounts pass the filter; accounts of the wrong program or size do not                                                            | Restating the filter struct's fields                       |

If no independent source exists, mark the test `TODO(verify)`; that code path
does not ship to mainnet. A green test is not verification.

Rounding tests push the boundary: inputs where floor vs. ceil differs by one
unit, values near `u64::MAX`, zero liquidity, tick/bin edges. Overflow must
return an error, not panic; a test asserting that is valuable.

## Junk patterns

The authoring gate rejects a new test that matches one; audits hunt for existing
tests that do.

- assertion-free coverage probes; tests that only assert "does not panic" when
  not panicking is not the contract;
- self-comparisons: a constant against the same literal, a `Clone` against its
  original, derived `PartialEq`/`Debug`/`thiserror` output against itself;
- **expected values produced by the function under test or a copy of it**
  (computing a quote with the same formula and comparing);
- copied inventories: DEX lists, program ID tables, offset lists restated in the
  test and compared against the code;
- exact source, import, or string greps;
- private helper or call-shape tests duplicated at the real boundary;
- duplicate invocations of the same contract under a different name;
- tests whose only purpose is keeping a test-only export, `pub`, or wrapper
  alive; dead production code whose only callers are tests;
- mocks that implement the asserted behavior (a fake gRPC stream doing the
  reconnect logic itself); one identical mock standing in for different APIs;
- fixtures that supply the ordering or slot the owner should produce;
- negative controls that pass for an unrelated reason: a different guard's
  error instead of the expected one, `is_err()` without checking the variant;
- names that promise more than the input exercises ("applies Token-2022
  transfer fee" using a mint without the fee extension).

## Retention bar

Keep a test when it independently guards:

- on-chain contracts: program ID, discriminator, PDA seeds, account layout and
  offsets, instruction account order and writable/signer flags, fee order,
  rounding direction, Token-2022 extensions, mint decimals;
- `checked_*` overflow returning an error;
- live-trading safety: dry-run as the default, limits and kill switch not
  bypassable, `minimum_amount_out` computation, simulation failure aborting;
- secret leakage: `rpc`/`grpc` error messages carrying no URL or token;
- config defaults and the parse contract;
- observable ordering (slot order, fork resolution, store update order);
- regressions with a credible failure mode.

**Fixtures are contracts.** The mainnet bytes under
`crates/dex/src/tests/fixtures/accounts/` are not the "copied fixtures" junk
pattern and are not deleted. New fixtures are captured only with
`scripts/capture_accounts.py`, never hand-made. After a program upgrade,
recapture the fixture; never loosen the decoder to fit it.

Static or slow is not a deletion reason. A test that resembles implementation
may still be the independent contract; prove otherwise before removing it. A
retained test that fails on the baseline is a likely product bug: reproduce it
and fix the owner rather than deleting the test.

## Audit mode

Discovery is read-only; report evidence before editing. Read the root
`AGENTS.md` first. Before judging a candidate, read the complete test, its
production owner, callers, sibling implementations (other DEX modules), and
overlapping tests.

Lanes for a broad sweep: `crates/dex`, `crates/market`, `crates/grpc` +
`crates/rpc`, `crates/domain` + `apps/`. Prefer a few high-confidence
candidates over a large speculative inventory.

Record every field below before editing. A missing field means the candidate is
not ready for deletion:

- exact test name and location;
- what failure it can actually detect;
- non-test callers of the covered production seam;
- stronger remaining proof, or why none is needed;
- why the test or seam exists (`git log -S`, `git blame`);
- production or test-support deletion it unlocks;
- risk and the focused validation command.

## Edit shape

Choose one coherent owner-boundary batch. Delete test-only `pub`,
`#[cfg(test)]` exports, and wrappers instead of leaving aliases. Move retained
regressions to their canonical owner (`src/tests.rs` or `src/tests/`).
Consolidate repeated per-DEX tests into one table-driven test.

Prefer net-negative production LOC. Do not add replacement tests that restate
the same implementation, and do not turn uncertain candidates into cleanup to
raise the deletion count.

## Validation

1. Narrowest run: `just test-crate <crate> <filter>` or
   `just test -p <crate> <filter>`. The whole workspace suite (`just ci`,
   `cargo nextest run --workspace`) never runs without human approval.
2. If doc tests changed: `cargo test -p <crate> --doc`.
3. `just lint` and `git diff --check`.
4. `git diff --numstat`; report production and test/test-support lines
   separately.

Commit, push, or open a PR only when explicitly asked.

## Handover

- removed low-value categories and root cause;
- production owner simplifications;
- retained false positives and why they stay;
- tests left as `TODO(verify)` for lack of an independent source;
- commands actually run; **tests not run are stated plainly**;
- production versus test LOC;
- follow-ups.
