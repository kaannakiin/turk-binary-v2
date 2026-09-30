set dotenv-load := true

# cargo-build-sbf 4.0.0's default; CI builds the router with the same platform-tools.
sbf_tools := "v1.53"

default:
    @just --list

check:
    cargo check --workspace --all-targets

fmt:
    cargo fmt --all

lint:
    cargo fmt --all --check
    cargo lint

test *args:
    cargo nextest run {{args}}

test-crate crate *args:
    cargo nextest run -p {{crate}} {{args}}

bench crate *args:
    cargo bench -p {{crate}} {{args}}

# Interleaved A/B of the route search bench: a git ref against the working tree (heavy).
bench-ab *args:
    python3 scripts/bench_ab.py {{args}}

deny:
    cargo deny check

ci: lint deny
    cargo nextest run --workspace --profile ci
    cargo test --workspace --doc

# The router program (onchain/, its own workspace).
lint-onchain:
    cargo fmt --manifest-path onchain/Cargo.toml --all --check
    cargo clippy --manifest-path onchain/Cargo.toml --workspace --all-targets --locked -- -D warnings

build-onchain:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml

test-onchain *args:
    cargo nextest run --manifest-path onchain/Cargo.toml {{args}}

# The replay pack rebuilt from this tree and run offline on its committed inputs; fails
# unless it reproduces the committed fixtures (scripts/replay_check.py). CI runs it.
replay-check *args:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/short-venue/Cargo.toml
    python3 scripts/replay_check.py {{args}}

# Uploads the bytecode programs.tsv pins as the release CI's replay job downloads. Run once
# after `just oracle` dumps programs whose hashes changed; a human runs it, never CI.
publish-oracle-programs:
    #!/usr/bin/env bash
    set -euo pipefail
    cd oracle/programs
    awk '{print $3"  "$1".so"}' programs.tsv | shasum -a 256 -c --strict
    digest="$(shasum -a 256 programs.tsv | cut -c1-64)"
    tag="oracle-programs-${digest:0:12}"
    gh release create "$tag" ./*.so programs.tsv --latest=false --title "$tag" \
        --notes "Mainnet program bytecode pinned by oracle/programs/programs.tsv (sha256 $digest), dumped by scripts/dump_programs.py. CI's replay job downloads it."

# LiteSVM: every paid swap in the selected venue replay corpus, sent through the router
# on the same accounts and mainnet bytecode. The default corpus is CPMM; pass the AMM v4
# corpus and an output path to record its replay too.
router-replay corpus="crates/quoter/src/tests/fixtures/svm/raydium_cpmm.json.gz" scenario_pools="crates/tx/src/tests/fixtures/scenario_pools.json.gz" out="crates/tx/src/tests/fixtures/router_replay.json":
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/short-venue/Cargo.toml
    ROUTER_CORPUS={{justfile_directory()}}/{{corpus}} ROUTER_PLANS={{justfile_directory()}}/target/router-plans.json cargo nextest run -p server --run-ignored only router_replay_plans --no-capture
    ROUTER_SCENARIO_PLANS={{justfile_directory()}}/target/router-scenario-plans.json cargo nextest run -p server --run-ignored only router_scenario_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router {{corpus}} target/router-plans.json oracle/programs onchain/target/deploy/router.so {{out}}
    cargo run --manifest-path oracle/Cargo.toml -- router-scenarios {{scenario_pools}} target/router-scenario-plans.json oracle/programs onchain/target/deploy/router.so onchain/target/deploy/short_venue.so crates/tx/src/tests/fixtures/router_scenarios.json

# Four-step split/merge with an intermediate Token-2022 transfer-fee branch.
# Direct venue swaps and both router forms replay on the same captured bank.
router-flow-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/short-venue/Cargo.toml
    ROUTER_FLOW_PLANS={{justfile_directory()}}/target/router-flow-plans.json cargo nextest run -p server --run-ignored only router_flow_plans
    cargo run --manifest-path oracle/Cargo.toml -- router crates/tx/src/tests/fixtures/scenario_pools.json.gz target/router-flow-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_flow_replay.json onchain/target/deploy/short_venue.so
    cargo nextest run -p tx split_merge_and_reused_cpmm_flows_match_direct_venue_execution

# LiteSVM: three-token AMM v4/CPMM routes, profitable cycles and a Token-2022 fee hop.
router-matrix-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_AMM_V4_MATRIX_PLANS={{justfile_directory()}}/target/router-amm-v4-matrix-plans.json ROUTER_AMM_V4_TOKEN22_PLANS={{justfile_directory()}}/target/router-amm-v4-token22-plans.json cargo nextest run -p server --run-ignored only router_amm_v4_ --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix oracle/snapshots/amm-v4-routes.json.gz target/router-amm-v4-matrix-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_amm_v4_matrix.json
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix oracle/snapshots/amm-v4-token22.json.gz target/router-amm-v4-token22-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_amm_v4_token22.json

# Same-slot CLMM/CPMM/AMM v4 paths, direct venue payouts, thresholds, and v1 budgets.
router-clmm-cross-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_CLMM_CROSS_PLANS={{justfile_directory()}}/target/router-clmm-cross-plans.json cargo nextest run -p server --run-ignored only router_clmm_cross_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/tx/src/tests/fixtures/clmm_cross_dex.json target/router-clmm-cross-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_clmm_cross.json

# Orca paid swaps through the router, compared with direct Whirlpool execution.
router-orca-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_CORPUS={{justfile_directory()}}/crates/quoter/src/tests/fixtures/svm/orca_whirlpool.json.gz ROUTER_PLANS={{justfile_directory()}}/target/router-orca-plans.json cargo nextest run -p server --run-ignored only router_replay_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router crates/quoter/src/tests/fixtures/svm/orca_whirlpool.json.gz target/router-orca-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_orca_replay.json

# DLMM paid swaps through the router, compared with direct Meteora execution.
router-dlmm-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_CORPUS={{justfile_directory()}}/crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz ROUTER_PLANS={{justfile_directory()}}/target/router-dlmm-plans.json cargo nextest run -p server --run-ignored only router_replay_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz target/router-dlmm-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_dlmm_replay.json

# Real DLMM Token-2022 transfer-fee pool, both directions and exact thresholds.
router-dlmm-fee-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_DLMM_FEE_PLANS={{justfile_directory()}}/target/router-dlmm-fee-plans.json cargo nextest run -p server --run-ignored only router_dlmm_fee_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/tx/src/tests/fixtures/dlmm_fee_pools.json target/router-dlmm-fee-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_dlmm_fee.json

# Live DLMM bitmap-extension pool, both directions against direct swap2.
router-dlmm-extension-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_DLMM_EXTENSION_PLANS={{justfile_directory()}}/target/router-dlmm-extension-plans.json cargo nextest run -p server --run-ignored only router_dlmm_extension_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/tx/src/tests/fixtures/dlmm_extension_pools.json target/router-dlmm-extension-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_dlmm_extension.json

# Same-slot DLMM→CLMM route with direct payouts and per-hop thresholds.
router-dlmm-cross-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_DLMM_CROSS_SNAPSHOT={{justfile_directory()}}/crates/tx/src/tests/fixtures/dlmm_cross_dex.json ROUTER_DLMM_CROSS_PLANS={{justfile_directory()}}/target/router-dlmm-cross-plans.json cargo nextest run -p server --run-ignored only router_dlmm_cross_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/tx/src/tests/fixtures/dlmm_cross_dex.json target/router-dlmm-cross-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_dlmm_cross.json

# Two consumed DLMM arrays; wrong, missing, and reversed CPI tails fail atomically.
router-dlmm-two-array-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_DLMM_TWO_ARRAY_PLANS={{justfile_directory()}}/target/router-dlmm-two-array-plans.json cargo nextest run -p server --run-ignored only router_dlmm_two_array_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz target/router-dlmm-two-array-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_dlmm_two_array.json

# Same-slot Orca/Raydium paths, direct venue payouts, thresholds, and v1 budgets.
router-orca-cross-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_ORCA_CROSS_SNAPSHOT={{justfile_directory()}}/crates/tx/src/tests/fixtures/orca_cross_dex.json ROUTER_ORCA_CROSS_PLANS={{justfile_directory()}}/target/router-orca-cross-plans.json cargo nextest run -p server --run-ignored only router_orca_cross_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/tx/src/tests/fixtures/orca_cross_dex.json target/router-orca-cross-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_orca_cross.json

# Same-slot Orca/CLMM two-hop cycle, thresholds and atomic rollback.
router-orca-cycle-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_ORCA_CYCLE_SNAPSHOT={{justfile_directory()}}/crates/tx/src/tests/fixtures/orca_cycle.json ROUTER_ORCA_CYCLE_PLANS={{justfile_directory()}}/target/router-orca-cycle-plans.json cargo nextest run -p server --run-ignored only router_orca_cycle_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/tx/src/tests/fixtures/orca_cycle.json target/router-orca-cycle-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_orca_cycle.json

# Real Whirlpool Token-2022 fee pools in both directions, direct program vs router.
router-orca-fee-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_ORCA_FEE_SNAPSHOT={{justfile_directory()}}/crates/tx/src/tests/fixtures/orca_fee_pools.json ROUTER_ORCA_FEE_PLANS={{justfile_directory()}}/target/router-orca-fee-plans.json cargo nextest run -p server --run-ignored only router_orca_fee_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/tx/src/tests/fixtures/orca_fee_pools.json target/router-orca-fee-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_orca_fee.json

# Two Token-2022 mints on a live Whirlpool, both directions and exact thresholds.
router-orca-pair-replay:
    NO_DNA=1 cargo build-sbf --tools-version {{sbf_tools}} --manifest-path onchain/programs/router/Cargo.toml
    ROUTER_ORCA_PAIR_SNAPSHOT={{justfile_directory()}}/crates/tx/src/tests/fixtures/orca_token22_pair.json ROUTER_ORCA_PAIR_PLANS={{justfile_directory()}}/target/router-orca-pair-plans.json cargo nextest run -p server --run-ignored only router_orca_pair_plans --no-capture
    cargo run --manifest-path oracle/Cargo.toml -- router-matrix crates/tx/src/tests/fixtures/orca_token22_pair.json target/router-orca-pair-plans.json oracle/programs onchain/target/deploy/router.so crates/tx/src/tests/fixtures/router_orca_pair.json

watch config="config.toml":
    cargo run -p turk-binary -- watch --config {{config}}

# `watch` plus POST /quote, /swap-instructions, /swap on `server.api_addr` and /health, /ready on
# `server.ops_addr`. Builds unsigned transactions; never signs or sends one.
serve config="config.toml":
    cargo run -p turk-binary -- serve --config {{config}}

# Read-only checks of what the gRPC provider supports, e.g. `just probe slots,clock`.
probe kinds="" config="config.toml":
    cargo run -p turk-binary -- probe --config {{config}} {{kinds}}

# Optimized build of `watch`, for profiling.
watch-release config="config.toml":
    cargo run --release -p turk-binary -- watch --config {{config}}

# Read-only: how a transaction's account writes and its status arrive on one stream.
txn-probe minutes="30" per_dex="20" config="config.toml":
    cargo run --release -p turk-binary -- txn-probe --config {{config}} --minutes {{minutes}} --per-dex {{per_dex}} --out target/txn-probe.tsv

# Read-only: record one DEX's pool streams as a replay fixture for crates/market.
txn-record dex minutes="1" per_dex="60" config="config.toml":
    cargo run --release -p turk-binary -- txn-probe --config {{config}} --minutes {{minutes}} --per-dex {{per_dex}} --only {{dex}} --record crates/market/src/tests/fixtures/streams/{{dex}}.tsv

# Read-only: ready pools' account views and the Clock, the input of the LiteSVM oracle (oracle/).
snapshot out="oracle/snapshots/latest.json.gz" per_dex="12" settle="90" config="config.toml":
    cargo run -p turk-binary -- snapshot --config {{config}} --out {{out}} --per-dex {{per_dex}} --settle-secs {{settle}}

# Read-only: every ready pool's view + Clock, the input of `test-universe` and `bench route`.
snapshot-universe out="oracle/snapshots/universe.json.gz" settle="90" config="config.toml":
    cargo run -p turk-binary -- snapshot --config {{config}} --out {{out}} --all --settle-secs {{settle}}

# Pruned against exhaustive route search on the `snapshot-universe` capture (release: it quotes a lot).
test-universe snapshot="oracle/snapshots/universe.json.gz":
    ROUTE_UNIVERSE={{justfile_directory()}}/{{snapshot}} cargo nextest run --release -p route --test snapshot --run-ignored only --no-capture

# Read-only, project RPC: Token-2022 mints whose older and newer transfer fees differ, for fixtures.
find-fee-mints want="5":
    python3 scripts/find_fee_mints.py {{want}}

# CPU-bound vanity search; writes an ignored program keypair with mode 0600.
grind-program-id prefix="TURK" threads="10" output="target/deploy/turk_binary-keypair.json":
    python3 scripts/grind_program_id.py --prefix "{{prefix}}" --threads "{{threads}}" --output "{{output}}"

# Read-only: dumps mainnet's deployed programs and runs the snapshot's swaps through them in LiteSVM.
# Writes the expected payouts to crates/quoter/src/tests/fixtures/svm.
oracle snapshot="oracle/snapshots/latest.json.gz":
    python3 scripts/dump_programs.py oracle/programs
    cargo run --manifest-path oracle/Cargo.toml -- {{snapshot}} oracle/programs crates/quoter/src/tests/fixtures/svm
