//! Runs every swap a quote test will check through the program bytecode
//! deployed on mainnet, over the account bytes of a `turk-binary snapshot`,
//! and records what the program paid.
//!
//! Usage: oracle SNAPSHOT PROGRAMS_DIR OUT_DIR

mod router;
mod rpc;
mod snapshot;
mod svm;
mod venue;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use arb_swap_ix::layout::meteora_dlmm::derive_meteora_dlmm_bitmap_extension_pda;
use arb_swap_ix::layout::raydium_clmm::derive_raydium_clmm_bitmap_extension_pda;
use arb_swap_ix::registry::RAYDIUM_CLMM_PROGRAM_ID;
use arb_swap_ix::{
    AccountResolver, BootLayout, PoolStaticAccounts, SwapAccountCtx, SwapHopContext, build_hop_ix,
};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;
use sha2::{Digest, Sha256};
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use snapshot::{Account, Clock, Pool, Stored};
use svm::Machine;
use venue::Venue;

const TOKEN: Pubkey = Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN_2022: Pubkey = Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const SYSTEM: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");
const COMPUTE_BUDGET: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");
const TOKEN_ACCOUNT_LEN: usize = 165;
// src: raydium-io/raydium-clmm programs/amm/src/instructions/swap_v2.rs (SwapSingleV2: 13 accounts before the remaining ones)
const CLMM_FIXED_ACCOUNTS: usize = 13;
// src: MeteoraAg/dlmm-sdk idls/dlmm.json (swap/swap2: bin_array_bitmap_extension is the second account)
const DLMM_EXTENSION_SLOT: usize = 1;
/// Fractions of the input reserve, as divisors, plus a fixed ladder: a
/// shared vault (DAMM v1) holds far more than any one pool's share.
const RESERVE_DIVISORS: [u64; 5] = [10_000, 1_000, 100, 10, 3];
const LADDER: [u64; 4] = [1_000, 1_000_000, 1_000_000_000, 1_000_000_000_000];
const FIXED_AMOUNT: u64 = 1_000_000;

struct Atas;

impl AccountResolver for Atas {
    fn ata(&self, owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
        svm::ata(owner, token_program, mint)
    }
}

#[derive(Serialize)]
struct Provenance {
    snapshot: String,
    snapshot_sha256: String,
    litesvm: &'static str,
    programs: Vec<String>,
}

#[derive(Serialize)]
struct Case {
    pool: String,
    input_mint: String,
    amount_in: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    arrays: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    out: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct Fixture<'a> {
    provenance: &'a Provenance,
    clock: &'a Clock,
    pools: Vec<&'a Pool>,
    extra: Vec<Account>,
    cases: Vec<Case>,
}

struct Hop<'a> {
    pool: Pubkey,
    venue: &'a Venue,
    accounts: &'a HashMap<Pubkey, Option<Stored>>,
    input: Pubkey,
    output: Pubkey,
}

impl Hop<'_> {
    fn program_of(&self, mint: &Pubkey) -> Pubkey {
        self.accounts
            .get(mint)
            .and_then(Option::as_ref)
            .map_or(TOKEN, |s| s.owner)
    }

    fn build(&self, payer: Pubkey, amount_in: u64) -> Result<Instruction, String> {
        let exec = self.venue.exec(&self.input);
        let mint_program = |mint: &Pubkey| self.program_of(mint);
        let ctx = SwapHopContext {
            pool: self.pool,
            payer,
            input_mint: self.input,
            output_mint: self.output,
            amount_in,
            min_out: 0,
            mint_program: &mint_program,
            accounts: SwapAccountCtx {
                resolver: &Atas,
                pool_static: PoolStaticAccounts::derive(&self.venue.layout, &self.pool),
                hooks: None,
            },
        };
        let mut ix =
            build_hop_ix(&self.venue.layout, &exec.state, &ctx).map_err(|e| e.to_string())?;
        self.pass_bitmap_extension(&mut ix);
        Ok(ix)
    }

    /// Both programs take the extension as an optional account and search it
    /// only when the swap leaves the pool's own bitmap; the builders pass it
    /// only for arrays they know sit outside. Passing it whenever it exists
    /// is always accepted.
    fn pass_bitmap_extension(&self, ix: &mut Instruction) {
        let exists = |key: &Pubkey| self.accounts.get(key).is_some_and(Option::is_some);
        match &self.venue.layout {
            BootLayout::RaydiumClmm { .. } => {
                let ext =
                    derive_raydium_clmm_bitmap_extension_pda(&self.pool, &RAYDIUM_CLMM_PROGRAM_ID)
                        .expect("extension address");
                if exists(&ext) && !ix.accounts.iter().any(|m| m.pubkey == ext) {
                    ix.accounts
                        .insert(CLMM_FIXED_ACCOUNTS, AccountMeta::new(ext, false));
                }
            }
            BootLayout::MeteoraDlmm { .. } => {
                let ext = derive_meteora_dlmm_bitmap_extension_pda(&self.pool)
                    .expect("extension address");
                if exists(&ext) {
                    ix.accounts[DLMM_EXTENSION_SLOT] = AccountMeta::new(ext, false);
                }
            }
            _ => {}
        }
    }

    /// The largest input-mint token account the swap writes that is not the
    /// user's: the pool's side of the trade.
    fn reserve(&self, ix: &Instruction, payer: &Pubkey) -> u64 {
        let user = svm::ata(payer, &self.program_of(&self.input), &self.input);
        ix.accounts
            .iter()
            .filter(|meta| meta.is_writable && meta.pubkey != user)
            .filter_map(|meta| self.accounts.get(&meta.pubkey)?.as_ref())
            .filter(|s| {
                (s.owner == TOKEN || s.owner == TOKEN_2022)
                    && s.data.len() >= TOKEN_ACCOUNT_LEN
                    && s.data[..32] == self.input.to_bytes()
            })
            .map(|s| u64::from_le_bytes(s.data[64..72].try_into().expect("amount")))
            .max()
            .unwrap_or(0)
    }
}

fn amounts(reserve: u64) -> Vec<u64> {
    let mut amounts: Vec<u64> = RESERVE_DIVISORS
        .iter()
        .map(|d| reserve / d)
        .chain(LADDER)
        .filter(|a| *a > 0)
        .collect();
    amounts.sort_unstable();
    amounts.dedup();
    amounts
}

struct Runner {
    machine: Machine,
    skip: HashSet<Pubkey>,
    fetched: HashMap<Pubkey, Option<Stored>>,
}

impl Runner {
    fn extras(&mut self, ix: &Instruction, pool: &HashMap<Pubkey, Option<Stored>>) -> Vec<Pubkey> {
        let wanted: Vec<Pubkey> = ix
            .accounts
            .iter()
            .map(|m| m.pubkey)
            .filter(|k| {
                !pool.contains_key(k)
                    && !self.skip.contains(k)
                    && !k.to_string().starts_with("Sysvar")
            })
            .collect();
        let missing: Vec<Pubkey> = wanted
            .iter()
            .copied()
            .filter(|k| !self.fetched.contains_key(k))
            .collect();
        if !missing.is_empty() {
            self.fetched.extend(rpc::fetch(&missing));
        }
        wanted
    }

    fn run(
        &mut self,
        hop: &Hop<'_>,
        clock: &Clock,
        extra: &mut BTreeMap<Pubkey, Option<Stored>>,
    ) -> Vec<Case> {
        let payer = self.machine.payer();
        let base = |amount_in: u64, arrays: Option<u8>, result: Result<u64, String>| Case {
            pool: hop.pool.to_string(),
            input_mint: hop.input.to_string(),
            amount_in: amount_in.to_string(),
            arrays,
            out: result.as_ref().ok().map(ToString::to_string),
            error: result.err(),
        };
        let arrays = hop.venue.exec(&hop.input).arrays;
        let probe = match hop.build(payer, FIXED_AMOUNT) {
            Ok(ix) => ix,
            Err(e) => return vec![base(FIXED_AMOUNT, arrays, Err(format!("build: {e}")))],
        };
        let needed = self.extras(&probe, hop.accounts);
        let extras: HashMap<Pubkey, Option<Stored>> = needed
            .iter()
            .map(|k| (*k, self.fetched[k].clone()))
            .collect();
        extra.extend(extras.clone());
        let mut cases = Vec::new();
        for amount_in in amounts(hop.reserve(&probe, &payer)) {
            self.machine.set_clock(clock);
            self.machine.load(hop.accounts);
            self.machine.load(&extras);
            let result = self
                .machine
                .fund(
                    (&hop.input, &hop.program_of(&hop.input)),
                    (&hop.output, &hop.program_of(&hop.output)),
                    amount_in,
                )
                .and_then(|destination| {
                    let ix = hop.build(payer, amount_in)?;
                    self.machine.swap(ix)?;
                    Ok(self.machine.balance(&destination))
                });
            cases.push(base(amount_in, arrays, result));
        }
        cases
    }
}

fn write(path: &Path, fixture: &Fixture<'_>) {
    let file = std::fs::File::create(path).expect("creating the fixture");
    let mut gz = GzEncoder::new(file, Compression::best());
    serde_json::to_writer(&mut gz, fixture).expect("writing the fixture");
    gz.finish().expect("gzip").flush().expect("flush");
}

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args.first().is_some_and(|mode| mode.as_os_str() == "router") {
        return router::main(&args[1..]);
    }
    let [snapshot_path, programs, out_dir] = args.as_slice() else {
        eprintln!("usage: oracle SNAPSHOT PROGRAMS_DIR OUT_DIR");
        std::process::exit(2);
    };
    let (snapshot, raw) = snapshot::load(snapshot_path);
    let provenance = Provenance {
        snapshot: snapshot_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        snapshot_sha256: format!("{:x}", Sha256::digest(&raw)),
        litesvm: "0.16.0",
        programs: std::fs::read_to_string(programs.join("programs.tsv"))
            .expect("programs.tsv")
            .lines()
            .map(str::to_owned)
            .collect(),
    };
    let rent = rpc::fetch(&[svm::RENT])
        .remove(&svm::RENT)
        .flatten()
        .expect("Rent sysvar");
    let machine = Machine::new(programs, &rent.data);
    let mut skip: HashSet<Pubkey> = machine.programs.iter().copied().collect();
    skip.extend([SYSTEM, COMPUTE_BUDGET, machine.payer()]);
    let mut runner = Runner {
        machine,
        skip,
        fetched: HashMap::new(),
    };
    let mut by_dex: BTreeMap<&str, Vec<&Pool>> = BTreeMap::new();
    for pool in &snapshot.pools {
        by_dex.entry(pool.dex.as_str()).or_default().push(pool);
    }
    std::fs::create_dir_all(out_dir).expect("out dir");
    for (dex, pools) in by_dex {
        let mut cases = Vec::new();
        let mut extra = BTreeMap::new();
        for pool in &pools {
            let address = pool.address();
            let accounts = pool.accounts();
            let venue = match Venue::new(dex, &address, &accounts) {
                Ok(venue) => venue,
                Err(e) => {
                    eprintln!("{dex} {address}: {e}");
                    continue;
                }
            };
            let (a, b) = venue.layout.mints();
            for (input, output) in [(a, b), (b, a)] {
                let hop = Hop {
                    pool: address,
                    venue: &venue,
                    accounts: &accounts,
                    input,
                    output,
                };
                cases.extend(runner.run(&hop, &snapshot.clock, &mut extra));
            }
        }
        let paid = cases.iter().filter(|c| c.out.is_some()).count();
        eprintln!(
            "{dex}: {} pools, {} cases, {paid} paid",
            pools.len(),
            cases.len()
        );
        let fixture = Fixture {
            provenance: &provenance,
            clock: &snapshot.clock,
            pools,
            extra: extra
                .iter()
                .map(|(key, stored)| Account::from_stored(key, stored.as_ref()))
                .collect(),
            cases,
        };
        write(&out_dir.join(format!("{dex}.json.gz")), &fixture);
    }
}
