use std::sync::Arc;

use ahash::HashMap;
use parking_lot::RwLock;
use solana_instruction::AccountMeta;
use solana_pubkey::Pubkey;

use crate::hook::{extra_account_metas_pda, parse_validation_account, resolve_hook_metas};
use crate::swap_ix::HookAccounts;

pub type HookWindowMetas = Arc<Vec<AccountMeta>>;

#[derive(Clone, Default)]
pub struct HookCache {
    inner: Arc<RwLock<HashMap<Pubkey, Option<HookWindowMetas>>>>,
}

impl HookCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn learn(
        &self,
        mint: &Pubkey,
        hook_program: &Pubkey,
        validation_data: Option<&[u8]>,
    ) -> Option<u8> {
        let resolved = validation_data.and_then(|data| {
            let window = parse_validation_account(data)?;
            if !window.deterministic {
                return None;
            }
            let metas = resolve_hook_metas(
                data,
                mint,
                hook_program,
                &extra_account_metas_pda(mint, hook_program),
            )?;
            (u8::try_from(metas.len()).ok()? == window.accounts).then_some(metas)
        });
        let width = resolved.as_ref().and_then(|m| u8::try_from(m.len()).ok());
        self.inner.write().insert(*mint, resolved.map(Arc::new));
        width
    }

    pub fn forget(&self, mint: &Pubkey) {
        self.inner.write().remove(mint);
    }

    pub fn window(&self, mint: &Pubkey) -> Option<Option<HookWindowMetas>> {
        self.inner.read().get(mint).cloned()
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Default)]
pub struct RouteHooks {
    windows: Vec<(Pubkey, HookWindowMetas)>,
}

impl RouteHooks {
    pub fn resolve(
        cache: &HookCache,
        mints: impl IntoIterator<Item = Pubkey>,
    ) -> Result<Self, Pubkey> {
        if cache.is_empty() {
            return Ok(Self::default());
        }
        let mut windows: Vec<(Pubkey, HookWindowMetas)> = Vec::new();
        for mint in mints {
            if windows.iter().any(|(m, _)| *m == mint) {
                continue;
            }
            match cache.window(&mint) {
                None => {}
                Some(None) => return Err(mint),
                Some(Some(metas)) => windows.push((mint, metas)),
            }
        }
        Ok(Self { windows })
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

impl HookAccounts for RouteHooks {
    fn metas(&self, mint: &Pubkey) -> &[AccountMeta] {
        self.windows
            .iter()
            .find(|(m, _)| m == mint)
            .map_or(&[][..], |(_, v)| v.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const META_LEN: usize = 35;

    fn validation(metas: &[(u8, [u8; 32])]) -> Vec<u8> {
        let mut out = crate::hook::EXECUTE_DISCRIMINATOR.to_vec();
        out.extend_from_slice(&((4 + metas.len() * META_LEN) as u32).to_le_bytes());
        out.extend_from_slice(&(metas.len() as u32).to_le_bytes());
        for (disc, cfg) in metas {
            out.push(*disc);
            out.extend_from_slice(cfg);
            out.push(0);
            out.push(0);
        }
        out
    }

    #[test]
    fn a_resolvable_window_is_cached_at_the_width_the_graph_will_budget() {
        let cache = HookCache::new();
        let (mint, hook) = (Pubkey::new_unique(), Pubkey::new_unique());
        let literal = Pubkey::new_unique();

        let width = cache.learn(&mint, &hook, Some(&validation(&[(0, literal.to_bytes())])));

        assert_eq!(width, Some(3));
        let metas = cache.window(&mint).unwrap().unwrap();
        assert_eq!(metas[0].pubkey, literal);
        assert_eq!(metas[1].pubkey, hook);
    }

    #[test]
    fn a_missing_validation_account_caches_a_refusal_rather_than_an_empty_window() {
        let cache = HookCache::new();
        let (mint, hook) = (Pubkey::new_unique(), Pubkey::new_unique());

        assert_eq!(cache.learn(&mint, &hook, None), None);
        assert_eq!(
            cache.window(&mint),
            Some(None),
            "the mint is known to be hooked and known to be unroutable, which is not \
             the same as carrying no hook"
        );
    }

    #[test]
    fn a_non_deterministic_window_is_refused_even_though_it_parses() {
        let cache = HookCache::new();
        let (mint, hook) = (Pubkey::new_unique(), Pubkey::new_unique());
        let mut cfg = [0u8; 32];
        cfg[0] = 4;
        cfg[1] = 1;

        assert_eq!(
            cache.learn(&mint, &hook, Some(&validation(&[(1, cfg)]))),
            None
        );
    }

    #[test]
    fn an_empty_cache_costs_a_route_one_read_and_no_lookups() {
        let cache = HookCache::new();
        let route =
            RouteHooks::resolve(&cache, [Pubkey::new_unique(), Pubkey::new_unique()]).unwrap();
        assert!(route.is_empty());
    }

    #[test]
    fn a_route_carries_each_hooked_mint_once_and_refuses_an_unroutable_one() {
        let cache = HookCache::new();
        let (hooked, plain, broken) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let hook = Pubkey::new_unique();
        cache.learn(
            &hooked,
            &hook,
            Some(&validation(&[(0, Pubkey::new_unique().to_bytes())])),
        );
        cache.learn(&broken, &hook, None);

        let route = RouteHooks::resolve(&cache, [hooked, plain, hooked]).unwrap();
        assert_eq!(route.metas(&hooked).len(), 3);
        assert!(route.metas(&plain).is_empty());

        assert_eq!(RouteHooks::resolve(&cache, [broken]).unwrap_err(), broken);
    }
}
