set dotenv-load := true

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

deny:
    cargo deny check

ci: lint deny
    cargo nextest run --workspace --profile ci
    cargo test --workspace --doc

watch config="config.toml":
    cargo run -p turk-binary -- watch --config {{config}}

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

# Read-only: dumps mainnet's deployed programs and runs the snapshot's swaps through them in LiteSVM.
# Writes the expected payouts to crates/quoter/src/tests/fixtures/svm.
oracle snapshot="oracle/snapshots/latest.json.gz":
    python3 scripts/dump_programs.py oracle/programs
    cargo run --manifest-path oracle/Cargo.toml -- {{snapshot}} oracle/programs crates/quoter/src/tests/fixtures/svm
