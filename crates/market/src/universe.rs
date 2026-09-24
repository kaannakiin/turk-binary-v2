use std::collections::{BTreeMap, BTreeSet};

use dex::{DexSpec, Discovery, MintSide, pump};
use domain::{AccountUpdate, DexKind, Pubkey};
use futures::future::try_join_all;
use rpc::RpcGateway;
use serde::Deserialize;

use crate::{MarketError, UniverseError};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UniverseConfig {
    #[serde(deserialize_with = "domain::serde_pubkey::vec::deserialize")]
    pub mints: Vec<Pubkey>,
    #[serde(deserialize_with = "domain::serde_pubkey::vec::deserialize")]
    pub pools: Vec<Pubkey>,
    pub allowed_dexes: Vec<DexKind>,
    pub blocked_dexes: Vec<DexKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolInfo {
    pub dex: DexKind,
    pub mints: Option<(Pubkey, Pubkey)>,
}

#[derive(Debug, Clone, Default)]
pub struct Universe {
    pub dexes: BTreeSet<DexKind>,
    pub pools: BTreeMap<Pubkey, PoolInfo>,
}

impl Universe {
    pub async fn resolve(config: &UniverseConfig, rpc: &RpcGateway) -> Result<Self, MarketError> {
        let dexes = effective_dexes(config)?;
        let mints: BTreeSet<Pubkey> = config.mints.iter().copied().collect();
        match (mints.len(), config.pools.is_empty()) {
            (0, true) => return Err(UniverseError::Empty.into()),
            (1, _) => return Err(UniverseError::SingleMint.into()),
            _ => {}
        }
        let mut pools = resolve_listed_pools(&config.pools, &dexes, rpc).await?;
        if !mints.is_empty() {
            for &kind in &dexes {
                for (pubkey, info) in discover(dex::spec(kind), &mints, rpc).await? {
                    pools.entry(pubkey).or_insert(info);
                }
            }
        }
        Ok(Self { dexes, pools })
    }

    #[must_use]
    pub fn pool_keys(&self) -> Vec<Pubkey> {
        self.pools.keys().copied().collect()
    }

    #[must_use]
    pub fn count_by_dex(&self) -> BTreeMap<DexKind, usize> {
        let mut counts = BTreeMap::new();
        for info in self.pools.values() {
            *counts.entry(info.dex).or_insert(0) += 1;
        }
        counts
    }
}

pub fn effective_dexes(config: &UniverseConfig) -> Result<BTreeSet<DexKind>, UniverseError> {
    if let Some(&dex) = config
        .allowed_dexes
        .iter()
        .find(|d| config.blocked_dexes.contains(d))
    {
        return Err(UniverseError::AllowedAndBlocked(dex));
    }
    if let Some(&dex) = config
        .allowed_dexes
        .iter()
        .find(|d| !dex::spec(**d).verified)
    {
        return Err(UniverseError::Unverified(dex));
    }
    let base: Vec<DexKind> = if config.allowed_dexes.is_empty() {
        DexKind::ALL
            .into_iter()
            .filter(|d| dex::spec(*d).verified)
            .collect()
    } else {
        config.allowed_dexes.clone()
    };
    let dexes: BTreeSet<_> = base
        .into_iter()
        .filter(|d| !config.blocked_dexes.contains(d))
        .collect();
    if dexes.is_empty() {
        return Err(UniverseError::NoDexEnabled);
    }
    Ok(dexes)
}

async fn resolve_listed_pools(
    pools: &[Pubkey],
    dexes: &BTreeSet<DexKind>,
    rpc: &RpcGateway,
) -> Result<BTreeMap<Pubkey, PoolInfo>, MarketError> {
    let accounts = rpc.get_multiple_accounts(pools).await?;
    let mut out = BTreeMap::new();
    for (&pool, account) in pools.iter().zip(accounts) {
        let account = account.ok_or(UniverseError::PoolMissing(pool))?;
        let dex =
            dex::identify(&account.owner, &account.data).ok_or(UniverseError::UnknownPool {
                pool,
                owner: account.owner,
            })?;
        if !dexes.contains(&dex) {
            return Err(UniverseError::PoolDexDisabled { pool, dex }.into());
        }
        let mints = dex::spec(dex).pool_mints(&account.data);
        out.insert(pool, PoolInfo { dex, mints });
    }
    Ok(out)
}

async fn discover(
    spec: &'static DexSpec,
    mints: &BTreeSet<Pubkey>,
    rpc: &RpcGateway,
) -> Result<Vec<(Pubkey, PoolInfo)>, MarketError> {
    let found = match spec.discovery {
        Discovery::ProgramAccounts => {
            // Both mints must be in the set, so querying side A for every
            // mint already reaches every pair; side B would only add duplicates.
            let queries = mints
                .iter()
                .filter_map(|m| spec.pool_filter_with_mint(MintSide::A, m))
                .map(|filter| async move { rpc.get_program_accounts(&filter).await });
            let accounts: Vec<AccountUpdate> = try_join_all(queries)
                .await
                .map_err(|source| discovery_error(spec, source))?
                .into_iter()
                .flatten()
                .collect();
            select_pair_pools(spec, mints, &accounts)
        }
        Discovery::MintPda => {
            let candidates: Vec<(Pubkey, Pubkey)> = mints
                .iter()
                .filter_map(|m| pump::bonding_curve_address(m).map(|pda| (*m, pda)))
                .collect();
            let pdas: Vec<Pubkey> = candidates.iter().map(|(_, pda)| *pda).collect();
            let accounts = rpc
                .get_multiple_accounts(&pdas)
                .await
                .map_err(|source| discovery_error(spec, source))?;
            candidates
                .into_iter()
                .zip(accounts)
                .filter_map(|((mint, pda), account)| {
                    let account = account?;
                    let pair = pump::active_bonding_curve_pair(&mint, &account.data)?;
                    (spec.is_pool(&account.owner, &account.data) && mints.contains(&pair.1))
                        .then_some((
                            pda,
                            PoolInfo {
                                dex: spec.kind,
                                mints: Some(pair),
                            },
                        ))
                })
                .collect()
        }
    };
    tracing::info!(dex = %spec.kind, pools = found.len(), "discovered pools");
    Ok(found)
}

const fn discovery_error(spec: &DexSpec, source: rpc::RpcError) -> MarketError {
    MarketError::Discovery {
        dex: spec.kind,
        source,
    }
}

fn select_pair_pools(
    spec: &DexSpec,
    mints: &BTreeSet<Pubkey>,
    accounts: &[AccountUpdate],
) -> Vec<(Pubkey, PoolInfo)> {
    let mut rejected = 0usize;
    let pools = accounts
        .iter()
        .filter_map(|account| {
            if !spec.is_pool(&account.owner, &account.data) {
                rejected += 1;
                return None;
            }
            let (a, b) = spec.pool_mints(&account.data)?;
            (a != b && mints.contains(&a) && mints.contains(&b)).then_some((
                account.pubkey,
                PoolInfo {
                    dex: spec.kind,
                    mints: Some((a, b)),
                },
            ))
        })
        .collect();
    if rejected > 0 {
        tracing::warn!(
            dex = %spec.kind,
            rejected,
            "accounts matched the pool filter but failed layout checks; the program may have been upgraded"
        );
    }
    pools
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use domain::{Slot, WriteVersion};

    use super::*;

    fn config(allowed: &[DexKind], blocked: &[DexKind]) -> UniverseConfig {
        UniverseConfig {
            allowed_dexes: allowed.to_vec(),
            blocked_dexes: blocked.to_vec(),
            ..UniverseConfig::default()
        }
    }

    #[test]
    fn empty_allowed_means_every_verified_dex() {
        let dexes = effective_dexes(&config(&[], &[])).unwrap();
        let verified: BTreeSet<_> = DexKind::ALL
            .into_iter()
            .filter(|d| dex::spec(*d).verified)
            .collect();
        assert_eq!(dexes, verified);
    }

    #[test]
    fn blocked_is_removed_from_allowed_all() {
        let dexes = effective_dexes(&config(&[], &[DexKind::PumpAmm])).unwrap();
        assert!(!dexes.contains(&DexKind::PumpAmm));
    }

    #[test]
    fn allowed_limits_the_set() {
        let dexes = effective_dexes(&config(&[DexKind::OrcaWhirlpool], &[])).unwrap();
        assert_eq!(dexes, BTreeSet::from([DexKind::OrcaWhirlpool]));
    }

    #[test]
    fn dex_in_both_lists_is_an_error() {
        let err = effective_dexes(&config(&[DexKind::RaydiumClmm], &[DexKind::RaydiumClmm]));
        assert!(matches!(
            err,
            Err(UniverseError::AllowedAndBlocked(DexKind::RaydiumClmm))
        ));
    }

    #[test]
    fn blocking_everything_is_an_error() {
        let err = effective_dexes(&config(&[], &DexKind::ALL));
        assert!(matches!(err, Err(UniverseError::NoDexEnabled)));
    }

    fn pool_account(spec: &DexSpec, mint_a: Pubkey, mint_b: Pubkey) -> AccountUpdate {
        let size = usize::try_from(spec.data_size.unwrap()).unwrap();
        let mut data = vec![0u8; size];
        if let Some(disc) = spec.discriminator {
            data[..8].copy_from_slice(&disc);
        }
        let (a, b) = spec.mint_offsets.unwrap();
        data[a..a + 32].copy_from_slice(mint_a.as_ref());
        data[b..b + 32].copy_from_slice(mint_b.as_ref());
        AccountUpdate {
            pubkey: Pubkey::new_unique(),
            owner: spec.program_id,
            lamports: 1,
            data: Bytes::from(data),
            slot: Slot(1),
            write_version: WriteVersion::SNAPSHOT,
        }
    }

    #[test]
    fn keeps_only_pools_with_both_mints_in_set() {
        let spec = dex::spec(DexKind::OrcaWhirlpool);
        let (x, y, z) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let mints = BTreeSet::from([x, y]);
        let inside = pool_account(spec, x, y);
        let outside = pool_account(spec, x, z);
        let selected = select_pair_pools(spec, &mints, &[inside.clone(), outside]);
        assert_eq!(
            selected.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            vec![inside.pubkey]
        );
    }

    #[test]
    fn drops_accounts_that_fail_layout_checks() {
        let spec = dex::spec(DexKind::OrcaWhirlpool);
        let (x, y) = (Pubkey::new_unique(), Pubkey::new_unique());
        let mut resized = pool_account(spec, x, y);
        resized.data = Bytes::from(vec![0u8; 10]);
        assert!(select_pair_pools(spec, &BTreeSet::from([x, y]), &[resized]).is_empty());
    }
}
