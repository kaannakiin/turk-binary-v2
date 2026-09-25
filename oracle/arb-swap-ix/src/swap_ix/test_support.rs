use ahash::HashMap;
use parking_lot::RwLock;
use solana_pubkey::Pubkey;
use spl_associated_token_account_interface::address::get_associated_token_address_with_program_id;

use super::AccountResolver;

pub struct SplAtas;

pub static ATAS: SplAtas = SplAtas;

impl AccountResolver for SplAtas {
    fn ata(&self, owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(owner, mint, token_program)
    }
}

#[derive(Default)]
pub struct MemoAtas {
    ata: RwLock<HashMap<(Pubkey, Pubkey, Pubkey), Pubkey>>,
    pda: RwLock<HashMap<(Pubkey, i64), Pubkey>>,
    wallet_pda: RwLock<HashMap<Pubkey, Pubkey>>,
}

impl AccountResolver for MemoAtas {
    fn ata(&self, owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
        let key = (*owner, *token_program, *mint);
        if let Some(addr) = self.ata.read().get(&key) {
            return *addr;
        }
        let addr = ATAS.ata(owner, token_program, mint);
        self.ata.write().insert(key, addr);
        addr
    }

    fn pda(
        &self,
        pool: &Pubkey,
        index: i64,
        compute: &dyn Fn() -> Option<Pubkey>,
    ) -> Option<Pubkey> {
        let key = (*pool, index);
        if let Some(p) = self.pda.read().get(&key) {
            return Some(*p);
        }
        let computed = compute()?;
        self.pda.write().insert(key, computed);
        Some(computed)
    }

    fn wallet_pda(&self, wallet: &Pubkey, compute: &dyn Fn() -> Pubkey) -> Pubkey {
        if let Some(p) = self.wallet_pda.read().get(wallet) {
            return *p;
        }
        let computed = compute();
        self.wallet_pda.write().insert(*wallet, computed);
        computed
    }
}
