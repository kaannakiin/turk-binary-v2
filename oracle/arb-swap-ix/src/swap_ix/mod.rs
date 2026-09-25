use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::kind::PoolKind;

pub const SYSVAR_INSTRUCTIONS_ID: Pubkey =
    Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");

/// Inline capacity mirrors quote-core's `EXEC_WINDOW_MAX`: a swap window never
/// carries more than eight tick/bin arrays, so these stay off the heap.
pub type WindowVec<T> = smallvec::SmallVec<[T; 8]>;

mod clmm_common;
pub mod dispatch;
mod meteora_damm_v1;
mod meteora_damm_v2;
mod meteora_dlmm;
mod pump_swap;
mod raydium_amm_v4;
mod raydium_clmm;
mod raydium_cpmm;
#[cfg(test)]
pub(crate) mod test_support;
mod whirlpool;

pub use meteora_damm_v1::MeteoraDammV1SwapIx;
pub use meteora_damm_v2::MeteoraDammV2SwapIx;
pub use meteora_dlmm::MeteoraDlmmSwapIx;
pub use pump_swap::{PumpSwapSwapIx, pump_coin_creator_vault_authority, pump_pool_v2_pda};
pub use raydium_amm_v4::RaydiumAmmV4SwapIx;
pub use raydium_clmm::RaydiumClmmSwapIx;
pub use raydium_cpmm::RaydiumCpmmSwapIx;
pub use whirlpool::WhirlpoolSwapIx;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStaticAccounts {
    pub whirlpool_oracle: Option<Pubkey>,
    pub clmm_bitmap_ext: Option<Pubkey>,
    pub dlmm_bitmap_ext: Option<Pubkey>,
    pub pump_coin_creator_vault_authority: Option<Pubkey>,
    pub pump_pool_v2: Option<Pubkey>,
}

impl PoolStaticAccounts {
    pub const EMPTY: Self = Self {
        whirlpool_oracle: None,
        clmm_bitmap_ext: None,
        dlmm_bitmap_ext: None,
        pump_coin_creator_vault_authority: None,
        pump_pool_v2: None,
    };

    pub fn derive(boot: &crate::layout::BootLayout, pool: &Pubkey) -> Self {
        use crate::layout::BootLayout as B;
        let mut s = Self::EMPTY;
        match boot {
            B::Whirlpool { .. } => {
                s.whirlpool_oracle = crate::layout::whirlpool::derive_whirlpool_oracle_pda(pool);
            }
            B::RaydiumClmm { .. } => {
                s.clmm_bitmap_ext =
                    crate::layout::raydium_clmm::derive_raydium_clmm_bitmap_extension_pda(
                        pool,
                        &crate::registry::RAYDIUM_CLMM_PROGRAM_ID,
                    );
            }
            B::MeteoraDlmm { .. } => {
                s.dlmm_bitmap_ext =
                    crate::layout::meteora_dlmm::derive_meteora_dlmm_bitmap_extension_pda(pool);
            }
            B::PumpSwap { layout } => {
                s.pump_coin_creator_vault_authority =
                    Some(pump_coin_creator_vault_authority(&layout.coin_creator));
                s.pump_pool_v2 = Some(pump_pool_v2_pda(&layout.base_mint));
            }
            _ => {}
        }
        s
    }
}

/// Per-mint Token-2022 transfer-hook accounts, already resolved.
///
/// Resolving them means reading the hook program's `ExtraAccountMetaList` and possibly
/// further accounts its seeds point at, which this crate cannot do —
/// `tests/no_send_guard.rs` forbids a client. They are resolved once upstream and
/// handed in.
pub trait HookAccounts {
    /// The hook block for `mint`, in `spl-transfer-hook-interface` order: resolved
    /// extras, then the hook program id, then the validation-state PDA. Empty when the
    /// mint has no live hook.
    fn metas(&self, mint: &Pubkey) -> &[AccountMeta];
}

pub trait AccountResolver {
    fn ata(&self, owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey;

    fn pda(
        &self,
        _pool: &Pubkey,
        _index: i64,
        compute: &dyn Fn() -> Option<Pubkey>,
    ) -> Option<Pubkey> {
        compute()
    }

    fn wallet_pda(&self, _wallet: &Pubkey, compute: &dyn Fn() -> Pubkey) -> Pubkey {
        compute()
    }
}

#[derive(Clone, Copy)]
pub struct SwapAccountCtx<'a> {
    pub resolver: &'a dyn AccountResolver,
    pub pool_static: PoolStaticAccounts,
    pub hooks: Option<&'a dyn HookAccounts>,
}

impl<'a> SwapAccountCtx<'a> {
    pub const fn new(resolver: &'a dyn AccountResolver) -> Self {
        Self {
            resolver,
            pool_static: PoolStaticAccounts::EMPTY,
            hooks: None,
        }
    }

    #[inline]
    pub fn hook_metas(&self, mint: &Pubkey) -> &'a [AccountMeta] {
        match self.hooks {
            Some(hooks) => hooks.metas(mint),
            None => &[],
        }
    }

    #[inline]
    pub fn ata(&self, wallet: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
        self.resolver.ata(wallet, token_program, mint)
    }

    #[inline]
    pub fn wallet_pda(&self, wallet: &Pubkey, compute: impl Fn() -> Pubkey) -> Pubkey {
        self.resolver.wallet_pda(wallet, &compute)
    }

    #[inline]
    pub fn pda(
        &self,
        pool: &Pubkey,
        index: i64,
        compute: impl Fn() -> Option<Pubkey>,
    ) -> Option<Pubkey> {
        self.resolver.pda(pool, index, &compute)
    }
}

pub struct SwapHopContext<'a> {
    pub pool: Pubkey,
    pub payer: Pubkey,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount_in: u64,
    pub min_out: u64,
    pub mint_program: &'a dyn Fn(&Pubkey) -> Pubkey,
    pub accounts: SwapAccountCtx<'a>,
}

impl SwapHopContext<'_> {
    #[inline]
    pub fn input_program(&self) -> Pubkey {
        (self.mint_program)(&self.input_mint)
    }

    #[inline]
    pub fn output_program(&self) -> Pubkey {
        (self.mint_program)(&self.output_mint)
    }
}

pub trait SwapIxBuilder {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SwapIxError {
    #[error("swap-ix: missing layout field `{0}`")]
    MissingField(&'static str),

    #[error("swap-ix: input_mint is not one of the pool's mints")]
    InvalidDirection,

    #[error("swap-ix: pool kind {0:?} is not buildable")]
    UnsupportedKind(PoolKind),

    #[error("swap-ix: empty tick/bin window — do not submit")]
    EmptyWindow,

    #[error("swap-ix: pool not swappable: {0}")]
    NotSwappable(&'static str),

    #[error("swap-ix: transfer-hook account slice exceeds the u8 length the venue encodes")]
    HookWindowTooWide,
}

#[cfg(test)]
mod static_accounts_tests {
    use super::*;
    use crate::layout::whirlpool::derive_whirlpool_oracle_pda;
    use crate::layout::{
        BootLayout, MeteoraDlmmLayout, PumpSwapLayout, RaydiumClmmLayout, WhirlpoolLayout,
    };

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    #[test]
    fn derive_whirlpool_oracle_eq_inline() {
        let pool = pk(7);
        let boot = BootLayout::Whirlpool {
            layout: WhirlpoolLayout::default(),
        };
        let s = PoolStaticAccounts::derive(&boot, &pool);
        assert_eq!(s.whirlpool_oracle, derive_whirlpool_oracle_pda(&pool));
        assert!(s.whirlpool_oracle.is_some());
        assert!(s.clmm_bitmap_ext.is_none());
    }

    #[test]
    fn derive_clmm_bitmap_ext_eq_inline() {
        let pool = pk(7);
        let boot = BootLayout::RaydiumClmm {
            layout: RaydiumClmmLayout::default(),
        };
        let s = PoolStaticAccounts::derive(&boot, &pool);
        assert_eq!(
            s.clmm_bitmap_ext,
            crate::layout::raydium_clmm::derive_raydium_clmm_bitmap_extension_pda(
                &pool,
                &crate::registry::RAYDIUM_CLMM_PROGRAM_ID,
            )
        );
        assert!(s.clmm_bitmap_ext.is_some());
    }

    #[test]
    fn derive_meteora_dlmm_bitmap_ext_eq_inline() {
        let pool = pk(7);
        let boot = BootLayout::MeteoraDlmm {
            layout: MeteoraDlmmLayout::default(),
        };
        let s = PoolStaticAccounts::derive(&boot, &pool);
        assert_eq!(
            s.dlmm_bitmap_ext,
            crate::layout::meteora_dlmm::derive_meteora_dlmm_bitmap_extension_pda(&pool)
        );
        assert!(s.dlmm_bitmap_ext.is_some());
        assert!(s.clmm_bitmap_ext.is_none());
    }

    #[test]
    fn derive_pumpswap_creator_vault_eq_inline() {
        let coin_creator = pk(14);
        let boot = BootLayout::PumpSwap {
            layout: PumpSwapLayout {
                coin_creator,
                ..PumpSwapLayout::default()
            },
        };
        let s = PoolStaticAccounts::derive(&boot, &pk(1));
        assert_eq!(
            s.pump_coin_creator_vault_authority,
            Some(pump_coin_creator_vault_authority(&coin_creator))
        );
    }

    #[test]
    fn kinds_without_static_accounts_derive_empty() {
        let boot = BootLayout::RaydiumCpmm {
            layout: crate::layout::RaydiumCpmmLayout::default(),
        };
        assert_eq!(
            PoolStaticAccounts::derive(&boot, &pk(1)),
            PoolStaticAccounts::EMPTY
        );
    }
}
