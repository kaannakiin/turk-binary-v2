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

deny:
    cargo deny check

ci: lint deny
    cargo nextest run --workspace --profile ci
    cargo test --workspace --doc
