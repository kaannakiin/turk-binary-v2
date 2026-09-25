use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use dex::{Closure, Dependency, Need, Scope};
use domain::{DexKind, Pubkey};
use grpc::{Group, GroupChange, GroupKey, Placement};

#[derive(Debug, Clone)]
pub(crate) struct PoolEntry {
    pub dex: DexKind,
    pub closure: Closure,
    pub deps: Arc<[Dependency]>,
}

#[derive(Debug, Default)]
pub(crate) struct Delta {
    pub changes: Vec<GroupChange>,
    pub added: Vec<Pubkey>,
    pub removed: Vec<Pubkey>,
}

#[derive(Debug, Default)]
struct KeyRefs {
    pools: BTreeMap<Pubkey, Dependency>,
    shared_votes: usize,
}

impl KeyRefs {
    /// A key two pools depend on is shared even if each calls it
    /// pool-scoped: it must live on one stream, not on both pools' shards.
    fn is_shared(&self) -> bool {
        self.shared_votes > 0 || self.pools.len() > 1
    }
}

#[derive(Debug, Default)]
pub(crate) struct ClosureIndex {
    pools: HashMap<Pubkey, PoolEntry>,
    keys: HashMap<Pubkey, KeyRefs>,
    include_swap: bool,
}

impl ClosureIndex {
    pub(crate) fn new(include_swap: bool) -> Self {
        Self {
            include_swap,
            ..Self::default()
        }
    }

    pub(crate) fn normalize(&self, mut closure: Closure) -> Closure {
        if !self.include_swap {
            closure.deps.retain(|d| d.need == Need::Quote);
        }
        closure
    }

    pub(crate) fn set_pool(&mut self, pool: Pubkey, dex: DexKind, closure: Closure) -> Delta {
        let closure = self.normalize(closure);
        let new = by_key(&closure.deps);
        let deps = Arc::from(closure.deps.as_slice());
        self.replace(pool, &new, Some(PoolEntry { dex, closure, deps }))
    }

    #[cfg(test)]
    pub(crate) fn remove_pool(&mut self, pool: &Pubkey) -> Delta {
        self.replace(*pool, &BTreeMap::new(), None)
    }

    fn replace(
        &mut self,
        pool: Pubkey,
        new: &BTreeMap<Pubkey, Dependency>,
        entry: Option<PoolEntry>,
    ) -> Delta {
        let old = self
            .pools
            .get(&pool)
            .map(|e| by_key(&e.closure.deps))
            .unwrap_or_default();
        let touched: BTreeSet<Pubkey> = old.keys().chain(new.keys()).copied().collect();
        let was_shared: HashMap<Pubkey, bool> =
            touched.iter().map(|k| (*k, self.is_shared(k))).collect();
        let was_known: BTreeSet<Pubkey> = touched
            .iter()
            .filter(|k| self.contains(k))
            .copied()
            .collect();
        for (key, dep) in &old {
            if let Some(refs) = self.keys.get_mut(key) {
                refs.pools.remove(&pool);
                refs.shared_votes -= usize::from(dep.scope == Scope::Shared);
                if refs.pools.is_empty() {
                    self.keys.remove(key);
                }
            }
        }
        for (key, dep) in new {
            let refs = self.keys.entry(*key).or_default();
            refs.pools.insert(pool, *dep);
            refs.shared_votes += usize::from(dep.scope == Scope::Shared);
        }
        match entry {
            Some(entry) => self.pools.insert(pool, entry),
            None => self.pools.remove(&pool),
        };

        let mut changes = Vec::new();
        let mut affected = BTreeSet::from([pool]);
        for key in &touched {
            let before = was_shared[key];
            let now = self.is_shared(key);
            if before == now {
                continue;
            }
            changes.push(if now {
                shared_upsert(*key)
            } else {
                GroupChange::Remove {
                    key: GroupKey(*key),
                    placement: Placement::Shared,
                }
            });
            affected.extend(
                self.keys
                    .get(key)
                    .into_iter()
                    .flat_map(|r| r.pools.keys().copied()),
            );
        }
        for pool in affected {
            changes.push(match self.pools.get(&pool) {
                Some(_) => GroupChange::Upsert {
                    key: GroupKey(pool),
                    placement: Placement::Pool,
                    group: self.pool_group(&pool),
                },
                None => GroupChange::Remove {
                    key: GroupKey(pool),
                    placement: Placement::Pool,
                },
            });
        }
        let now_known: BTreeSet<Pubkey> = touched
            .iter()
            .filter(|k| self.contains(k))
            .copied()
            .collect();
        Delta {
            changes,
            added: now_known.difference(&was_known).copied().collect(),
            removed: was_known.difference(&now_known).copied().collect(),
        }
    }

    fn pool_group(&self, pool: &Pubkey) -> Group {
        let pubkeys = self
            .pools
            .get(pool)
            .into_iter()
            .flat_map(|e| e.closure.deps.iter())
            .map(|d| d.pubkey)
            .filter(|k| !self.is_shared(k))
            .collect();
        Group {
            pubkeys,
            filters: Vec::new(),
        }
    }

    pub(crate) fn pool(&self, pool: &Pubkey) -> Option<&PoolEntry> {
        self.pools.get(pool)
    }

    pub(crate) fn pools_of(&self, key: &Pubkey) -> impl Iterator<Item = &Pubkey> {
        self.keys.get(key).into_iter().flat_map(|r| r.pools.keys())
    }

    /// Every pool that depends on `key`, with the dependency as that pool declared it.
    pub(crate) fn refs(&self, key: &Pubkey) -> impl Iterator<Item = (&Pubkey, &Dependency)> {
        self.keys.get(key).into_iter().flat_map(|r| r.pools.iter())
    }

    pub(crate) fn is_shared(&self, key: &Pubkey) -> bool {
        self.keys.get(key).is_some_and(KeyRefs::is_shared)
    }

    pub(crate) fn contains(&self, key: &Pubkey) -> bool {
        self.keys.contains_key(key)
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &Pubkey> {
        self.keys.keys()
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }
}

fn by_key(deps: &[Dependency]) -> BTreeMap<Pubkey, Dependency> {
    let mut out: BTreeMap<Pubkey, Dependency> = BTreeMap::new();
    for dep in deps {
        let merged = out.entry(dep.pubkey).or_insert(*dep);
        if dep.scope == Scope::Shared {
            merged.scope = Scope::Shared;
        }
    }
    out
}

fn shared_upsert(key: Pubkey) -> GroupChange {
    GroupChange::Upsert {
        key: GroupKey(key),
        placement: Placement::Shared,
        group: Group {
            pubkeys: BTreeSet::from([key]),
            filters: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use dex::{OwnerRule, Role};

    use super::*;

    const POOL_A: Pubkey = Pubkey::new_from_array([1; 32]);
    const POOL_B: Pubkey = Pubkey::new_from_array([2; 32]);
    const MINT: Pubkey = Pubkey::new_from_array([3; 32]);
    const VAULT: Pubkey = Pubkey::new_from_array([4; 32]);

    fn dep(pubkey: Pubkey, scope: Scope) -> Dependency {
        Dependency::new(pubkey, Role::Clock, scope, OwnerRule::Sysvar)
    }

    fn closure(deps: Vec<Dependency>) -> Closure {
        Closure {
            deps,
            awaiting: Vec::new(),
            verified: true,
        }
    }

    fn with_pool(pool: Pubkey, extra: &[Dependency]) -> Closure {
        let mut deps = vec![dep(pool, Scope::Pool)];
        deps.extend_from_slice(extra);
        closure(deps)
    }

    fn shared_changes(changes: &[GroupChange]) -> Vec<(bool, Pubkey)> {
        changes
            .iter()
            .filter_map(|c| match c {
                GroupChange::Upsert {
                    key,
                    placement: Placement::Shared,
                    ..
                } => Some((true, key.0)),
                GroupChange::Remove {
                    key,
                    placement: Placement::Shared,
                } => Some((false, key.0)),
                _ => None,
            })
            .collect()
    }

    fn pool_group(changes: &[GroupChange], pool: Pubkey) -> Option<&Group> {
        changes.iter().find_map(|c| match c {
            GroupChange::Upsert {
                key,
                placement: Placement::Pool,
                group,
            } if key.0 == pool => Some(group),
            _ => None,
        })
    }

    #[test]
    fn a_shared_key_is_subscribed_once_and_released_with_its_last_pool() {
        let mut index = ClosureIndex::new(true);
        let first = index.set_pool(
            POOL_A,
            DexKind::RaydiumCpmm,
            with_pool(POOL_A, &[dep(MINT, Scope::Shared)]),
        );
        let second = index.set_pool(
            POOL_B,
            DexKind::RaydiumCpmm,
            with_pool(POOL_B, &[dep(MINT, Scope::Shared)]),
        );
        let drop_a = index.remove_pool(&POOL_A);
        let drop_b = index.remove_pool(&POOL_B);
        assert_eq!(
            [
                shared_changes(&first.changes),
                shared_changes(&second.changes),
                shared_changes(&drop_a.changes),
                shared_changes(&drop_b.changes)
            ],
            [vec![(true, MINT)], vec![], vec![], vec![(false, MINT)]]
        );
    }

    #[test]
    fn pool_groups_hold_only_pool_scoped_keys() {
        let mut index = ClosureIndex::new(true);
        let changes = index.set_pool(
            POOL_A,
            DexKind::RaydiumAmmV4,
            with_pool(POOL_A, &[dep(VAULT, Scope::Pool), dep(MINT, Scope::Shared)]),
        );
        assert_eq!(
            pool_group(&changes.changes, POOL_A).unwrap().pubkeys,
            BTreeSet::from([POOL_A, VAULT])
        );
    }

    #[test]
    fn a_pool_scoped_key_needed_by_two_pools_moves_to_the_shared_stream() {
        let mut index = ClosureIndex::new(true);
        index.set_pool(
            POOL_A,
            DexKind::RaydiumAmmV4,
            with_pool(POOL_A, &[dep(VAULT, Scope::Pool)]),
        );
        let changes = index.set_pool(
            POOL_B,
            DexKind::RaydiumAmmV4,
            with_pool(POOL_B, &[dep(VAULT, Scope::Pool)]),
        );
        let a = pool_group(&changes.changes, POOL_A).unwrap();
        assert_eq!(
            (shared_changes(&changes.changes), a.pubkeys.contains(&VAULT)),
            (vec![(true, VAULT)], false)
        );
    }

    #[test]
    fn removing_a_pool_removes_its_group() {
        let mut index = ClosureIndex::new(true);
        index.set_pool(POOL_A, DexKind::RaydiumAmmV4, with_pool(POOL_A, &[]));
        let changes = index.remove_pool(&POOL_A);
        assert!(changes.changes.contains(&GroupChange::Remove {
            key: GroupKey(POOL_A),
            placement: Placement::Pool
        }));
    }

    #[test]
    fn swap_only_accounts_are_left_out_when_not_streamed() {
        let mut index = ClosureIndex::new(false);
        let changes = index.set_pool(
            POOL_A,
            DexKind::MeteoraDammV2,
            with_pool(POOL_A, &[dep(VAULT, Scope::Pool).swap_only()]),
        );
        assert!(
            !pool_group(&changes.changes, POOL_A)
                .unwrap()
                .pubkeys
                .contains(&VAULT)
        );
    }

    #[test]
    fn a_changed_closure_replaces_the_pool_group() {
        let mut index = ClosureIndex::new(true);
        index.set_pool(
            POOL_A,
            DexKind::RaydiumClmm,
            with_pool(POOL_A, &[dep(VAULT, Scope::Pool)]),
        );
        let array = Pubkey::new_unique();
        let changes = index.set_pool(
            POOL_A,
            DexKind::RaydiumClmm,
            with_pool(POOL_A, &[dep(array, Scope::Pool)]),
        );
        assert_eq!(
            (
                pool_group(&changes.changes, POOL_A)
                    .unwrap()
                    .pubkeys
                    .clone(),
                changes.added,
                changes.removed
            ),
            (BTreeSet::from([POOL_A, array]), vec![array], vec![VAULT])
        );
    }
}
