//! `arb-swap-ix` BUILDS instructions and must never gain the ability to sign or
//! send one, to touch the filesystem, or to depend on another crate in this
//! workspace.
//!
//! The first is the whole reason the crate is safe to have in a workspace whose
//! invariant is "never signs, never sends". A window builder that grew a keypair
//! or an RPC client would turn every measurement harness that links it into a
//! live trading path, silently.
//!
//! The last two are what let the directory be lifted into the arb-router repo and
//! published as an SDK unchanged. A single path dependency drags this workspace's
//! private git sources into whatever consumes it, and the wallet state that used
//! to live here now sits in `crates/ata` behind `AccountResolver`.
//!
//! Sibling of the orchestrator's guard, with the forbidden set adjusted to
//! this crate: no signer, no transaction assembly, no network.

use std::path::{Path, PathBuf};

const FORBIDDEN_DEPS: &[&str] = &[
    "solana-keypair",
    "solana-signer",
    "solana-transaction",
    "solana-client",
    "solana-rpc-client",
    "reqwest",
    "hyper",
    "tonic",
    "litesvm",
];

const FORBIDDEN_SRC_TOKENS: &[&str] = &[
    "Keypair",
    "sign_message",
    "try_sign",
    "send_transaction",
    "RpcClient",
    "VersionedTransaction",
];

fn mentions_crate(text: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    text.match_indices(name).any(|(i, _)| {
        let before_ok = !text[..i].chars().next_back().is_some_and(is_ident);
        let after_ok = !text[i + name.len()..].chars().next().is_some_and(is_ident);
        before_ok && after_ok
    })
}

fn runtime_dependency_tables(manifest: &str) -> String {
    let mut scoped = String::new();
    let mut in_scope = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_scope = trimmed == "[dependencies]"
                || trimmed == "[build-dependencies]"
                || trimmed.starts_with("[dependencies.")
                || trimmed.starts_with("[build-dependencies.")
                || trimmed.starts_with("[target.");
            if in_scope {
                scoped.push_str(trimmed);
                scoped.push('\n');
            }
            continue;
        }
        if in_scope {
            scoped.push_str(line);
            scoped.push('\n');
        }
    }
    scoped
}

fn walk_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn runtime_dependency_tables_never_mention_signing_or_network_crates() {
    let scoped = runtime_dependency_tables(include_str!("../Cargo.toml"));
    for name in FORBIDDEN_DEPS {
        assert!(
            !mentions_crate(&scoped, name),
            "arb-swap-ix builds instructions and must not be able to sign or send: \
             a runtime dependency table mentions `{name}`"
        );
    }
}

#[test]
fn dev_dependency_table_is_exempt_from_the_scan() {
    let manifest = "[dependencies]\nfoo = \"1\"\n[dev-dependencies]\nsolana-keypair = \"2\"\n[dependencies.bar]\nversion = \"1\"\n";
    let scoped = runtime_dependency_tables(manifest);
    assert!(!mentions_crate(&scoped, "solana-keypair"));
    assert!(mentions_crate(&scoped, "foo"));
    assert!(mentions_crate(&scoped, "bar"));
}

#[test]
fn target_scoped_dependency_table_with_a_forbidden_name_is_caught() {
    let manifest = "[dev-dependencies]\nx = \"1\"\n[target.'cfg(target_os = \"linux\")'.dependencies]\nsolana-signer = \"2\"\n";
    let scoped = runtime_dependency_tables(manifest);
    assert!(mentions_crate(&scoped, "solana-signer"));
}

#[test]
fn src_never_names_a_signing_or_sending_api() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk_rs(&src, &mut files);
    assert!(
        !files.is_empty(),
        "src/ walk found no .rs files — guard is vacuous"
    );
    let mut violations = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        for (number, line) in text.lines().enumerate() {
            for token in FORBIDDEN_SRC_TOKENS {
                if line.contains(token) {
                    violations.push(format!(
                        "{}:{}: `{token}` in: {}",
                        path.display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "arb-swap-ix must construct instructions and nothing else:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_crate_does_no_io_at_all() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk_rs(&src, &mut files);
    assert!(
        !files.is_empty(),
        "src/ walk found no .rs files — guard is vacuous"
    );
    let mut io_files: Vec<String> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        if text.contains("std::fs") || text.contains("use std::net") {
            io_files.push(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    io_files.sort();
    assert!(
        io_files.is_empty(),
        "arb-swap-ix constructs instructions and does no IO; found: {}",
        io_files.join(", ")
    );
}

#[test]
fn no_runtime_dependency_comes_from_this_workspace() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let siblings: Vec<String> = std::fs::read_dir(root.join("crates"))
        .expect("crates/ dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join("Cargo.toml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "arb-swap-ix")
        .collect();
    assert!(
        siblings.len() > 5,
        "sibling crate scan found only {siblings:?} — guard is vacuous"
    );

    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let scoped = runtime_dependency_tables(&manifest);
    let mut found: Vec<&String> = siblings
        .iter()
        .filter(|name| mentions_crate(&scoped, name))
        .collect();
    found.sort();
    assert!(
        found.is_empty(),
        "arb-swap-ix must stay liftable into the arb-router repo, but depends on: {found:?}"
    );
    assert!(
        !scoped.contains("path ="),
        "a path dependency appeared in arb-swap-ix's runtime tables"
    );
}
