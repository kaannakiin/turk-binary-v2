use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use dex::Dependency;
use domain::chain::CLOCK_SYSVAR;
use domain::{ChainClock, DexKind, Pubkey, Slot};
use tokio::sync::broadcast;

use crate::readiness::Readiness;
use crate::store::{AccountStore, Layer, StoredAccount};

#[derive(Debug, Clone)]
pub(crate) struct PoolMeta {
    pub dex: DexKind,
    pub deps: Arc<[Dependency]>,
    pub readiness: Readiness,
}

pub(crate) type PoolTable = HashMap<Pubkey, PoolMeta>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolChanged {
    pub pool: Pubkey,
    pub slot: Slot,
}

#[derive(Debug, Clone)]
pub struct PoolView {
    pub pool: Pubkey,
    pub dex: DexKind,
    pub readiness: Readiness,
    /// `None`: not known yet. An account with `lamports == 0` is confirmed
    /// absent (an optional array that does not exist).
    pub accounts: Vec<(Dependency, Option<StoredAccount>)>,
    pub clock: Option<ChainClock>,
}

impl PoolView {
    #[must_use]
    pub fn get(&self, role: dex::Role) -> Option<&StoredAccount> {
        self.accounts
            .iter()
            .find(|(d, _)| d.role == role)
            .and_then(|(_, a)| a.as_ref())
    }
}

/// Read side for the quote layer. Every account of a pool view is read
/// under one store lock, so they are one consistent moment of the store.
#[derive(Clone)]
pub struct MarketReader {
    pub(crate) store: Arc<AccountStore>,
    pub(crate) table: Arc<RwLock<PoolTable>>,
    pub(crate) changes: broadcast::Sender<PoolChanged>,
}

impl MarketReader {
    #[must_use]
    pub fn pools(&self) -> Vec<(Pubkey, DexKind, Readiness)> {
        self.table()
            .iter()
            .map(|(k, m)| (*k, m.dex, m.readiness))
            .collect()
    }

    #[must_use]
    pub fn readiness(&self, pool: &Pubkey) -> Option<Readiness> {
        self.table().get(pool).map(|m| m.readiness)
    }

    #[must_use]
    pub fn pool_view(&self, pool: &Pubkey, layer: Layer) -> Option<PoolView> {
        let meta = self.table().get(pool)?.clone();
        let mut keys: Vec<Pubkey> = meta.deps.iter().map(|d| d.pubkey).collect();
        keys.push(CLOCK_SYSVAR);
        let mut accounts = self.store.read_many(&keys, layer);
        let clock = accounts
            .pop()
            .flatten()
            .and_then(|a| ChainClock::decode(&a.data));
        Some(PoolView {
            pool: *pool,
            dex: meta.dex,
            readiness: meta.readiness,
            accounts: meta.deps.iter().copied().zip(accounts).collect(),
            clock,
        })
    }

    /// From the Clock sysvar, never the host clock: fees and activation use
    /// the chain's notion of time.
    #[must_use]
    pub fn clock(&self, layer: Layer) -> Option<ChainClock> {
        let account = match layer {
            Layer::Head => self.store.head(&CLOCK_SYSVAR),
            Layer::Committed => self.store.committed(&CLOCK_SYSVAR),
        }?;
        ChainClock::decode(&account.data)
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<PoolChanged> {
        self.changes.subscribe()
    }

    fn table(&self) -> std::sync::RwLockReadGuard<'_, PoolTable> {
        self.table.read().unwrap_or_else(PoisonError::into_inner)
    }
}
