//! `oracle surfpool CORPUS PLANS PROGRAMS_DIR ROUTER_SO OUT_DIR`: the accounts
//! `oracle router` prepares for each plan, as a Surfpool snapshot, with the
//! plan's v1 transaction signed by the replay payer, so
//! `scripts/surfpool_replay.py` can send it through a Surfnet's JSON-RPC.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use solana_loader_v3_interface::state::UpgradeableLoaderState;

use crate::router::{self, Corpus, Plans};
use crate::snapshot::Clock;
use crate::svm;

const CLOCK: solana_pubkey::Pubkey =
    solana_pubkey::Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SurfAccount {
    lamports: u64,
    owner: String,
    executable: bool,
    rent_epoch: u64,
    data: String,
}

#[derive(Serialize)]
struct Entry {
    name: String,
    snapshot: PathBuf,
    transaction: String,
    destination: String,
    expected_out: String,
}

#[derive(Serialize)]
struct Manifest<'a> {
    clock: &'a Clock,
    plans: Vec<Entry>,
}

pub fn main(args: &[PathBuf]) {
    let [corpus_path, plans_path, programs, router_so, out_dir] = args else {
        eprintln!("usage: oracle surfpool CORPUS PLANS PROGRAMS_DIR ROUTER_SO OUT_DIR");
        std::process::exit(2);
    };
    let (corpus, _) = Corpus::read(corpus_path);
    let plans: Plans = serde_json::from_slice(&std::fs::read(plans_path).expect("reading plans"))
        .expect("parsing plans");
    let router = std::fs::read(router_so).expect("reading the router program");
    let accounts = corpus.accounts();
    std::fs::create_dir_all(out_dir).expect("creating the output directory");
    let mut entries = Vec::new();
    for (index, plan) in plans.plans.iter().enumerate() {
        let mut machine = router::machine(programs, &router);
        let destination = router::prepare(&mut machine, &corpus.clock, &accounts, plan)
            .unwrap_or_else(|error| panic!("{}: {error}", plan.name));
        let unsigned = STANDARD
            .decode(&plan.transaction)
            .expect("base64 transaction");
        let signed = machine.sign(&unsigned).expect("signing the transaction");
        let mut keys: BTreeSet<_> = signed
            .message
            .static_account_keys()
            .iter()
            .copied()
            .collect();
        keys.extend([CLOCK, svm::RENT]);
        let program_data: Vec<_> = keys
            .iter()
            .filter(|key| {
                machine.account(key).is_some_and(|account| {
                    account.owner == svm::LOADER_V3
                        && matches!(
                            wincode::deserialize(&account.data),
                            Ok(UpgradeableLoaderState::Program { .. })
                        )
                })
            })
            .map(svm::program_data)
            .collect();
        keys.extend(program_data);
        let snapshot: BTreeMap<String, SurfAccount> = keys
            .iter()
            .filter_map(|key| {
                let account = machine.account(key)?;
                Some((
                    key.to_string(),
                    SurfAccount {
                        lamports: account.lamports,
                        owner: account.owner.to_string(),
                        executable: account.executable,
                        rent_epoch: account.rent_epoch,
                        data: STANDARD.encode(&account.data),
                    },
                ))
            })
            .collect();
        let path = out_dir.join(format!("{index}.json"));
        std::fs::write(
            &path,
            serde_json::to_vec(&snapshot).expect("serializing the snapshot"),
        )
        .expect("writing the snapshot");
        entries.push(Entry {
            name: if plan.name.is_empty() {
                index.to_string()
            } else {
                plan.name.clone()
            },
            snapshot: path,
            transaction: STANDARD.encode(wincode::serialize(&signed).expect("v1 bytes")),
            destination: destination.to_string(),
            expected_out: plan.expected_out.clone(),
        });
    }
    let manifest = Manifest {
        clock: &corpus.clock,
        plans: entries,
    };
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("serializing the manifest"),
    )
    .expect("writing the manifest");
    eprintln!(
        "{} plans written to {}",
        manifest.plans.len(),
        out_dir.display()
    );
}
