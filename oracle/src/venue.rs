//! What a transaction builder has to know per pool: the layout the swap
//! instruction reads and, for array venues, which arrays to pass and in
//! which order.

use std::collections::HashMap;

use arb_swap_ix::layout::meteora_damm_v1::{decode_damm_v1_vault, decode_meteora_damm_v1};
use arb_swap_ix::layout::meteora_damm_v2::decode_damm_v2;
use arb_swap_ix::layout::meteora_dlmm::{
    bin_array_index, decode_meteora_dlmm, read_bin_array_index,
};
use arb_swap_ix::layout::pump_swap::decode_pump_swap;
use arb_swap_ix::layout::raydium_amm_v4::decode_raydium_amm_v4;
use arb_swap_ix::layout::raydium_clmm::{
    RAYDIUM_CLMM_TICKS_PER_ARRAY, decode_raydium_clmm, derive_raydium_clmm_tick_array_pda,
    read_clmm_tick_array_start_index,
};
use arb_swap_ix::layout::raydium_cpmm::decode_raydium_cpmm;
use arb_swap_ix::layout::whirlpool::decode_whirlpool;
use arb_swap_ix::registry::{METEORA_DLMM_PROGRAM_ID, RAYDIUM_CLMM_PROGRAM_ID};
use arb_swap_ix::{BootLayout, HopExecState, WHIRLPOOL_SWAP_TICK_ARRAYS};
use solana_pubkey::Pubkey;

use crate::snapshot::Stored;

/// Arrays passed to one swap; also what the quote is allowed to cross.
pub const MAX_ARRAYS: usize = 8;

pub struct Venue {
    pub layout: BootLayout,
    arrays: Vec<(i64, Pubkey)>,
}

pub struct Exec {
    pub state: HopExecState,
    pub arrays: Option<u8>,
}

fn data<'a>(accounts: &'a HashMap<Pubkey, Option<Stored>>, key: &Pubkey) -> Option<&'a [u8]> {
    accounts.get(key)?.as_ref().map(|s| s.data.as_slice())
}

impl Venue {
    pub fn new(
        dex: &str,
        pool: &Pubkey,
        accounts: &HashMap<Pubkey, Option<Stored>>,
    ) -> Result<Self, String> {
        let bytes = data(accounts, pool).ok_or("pool account missing")?;
        let fail = |e: arb_swap_ix::PoolsError| e.to_string();
        let mut arrays = Vec::new();
        let layout = match dex {
            "raydium_amm_v4" => BootLayout::RaydiumAmmV4 {
                layout: decode_raydium_amm_v4(bytes).map_err(fail)?,
            },
            "raydium_cpmm" => BootLayout::RaydiumCpmm {
                layout: decode_raydium_cpmm(bytes).map_err(fail)?,
            },
            "raydium_clmm" => {
                for (key, stored) in accounts {
                    let Some(stored) = stored else { continue };
                    if stored.owner != RAYDIUM_CLMM_PROGRAM_ID || key == pool {
                        continue;
                    }
                    let Ok(start) = read_clmm_tick_array_start_index(&stored.data) else {
                        continue;
                    };
                    if derive_raydium_clmm_tick_array_pda(pool, start, &RAYDIUM_CLMM_PROGRAM_ID)
                        == Some(*key)
                    {
                        arrays.push((i64::from(start), *key));
                    }
                }
                BootLayout::RaydiumClmm {
                    layout: decode_raydium_clmm(bytes).map_err(fail)?,
                }
            }
            "orca_whirlpool" => BootLayout::Whirlpool {
                layout: decode_whirlpool(bytes).map_err(fail)?,
            },
            "meteora_dlmm" => {
                for (key, stored) in accounts {
                    let Some(stored) = stored else { continue };
                    if stored.owner != METEORA_DLMM_PROGRAM_ID || key == pool {
                        continue;
                    }
                    if let Ok(index) = read_bin_array_index(&stored.data) {
                        arrays.push((index, *key));
                    }
                }
                BootLayout::MeteoraDlmm {
                    layout: decode_meteora_dlmm(bytes).map_err(fail)?,
                }
            }
            "meteora_damm_v2" => BootLayout::MeteoraDammV2 {
                layout: decode_damm_v2(bytes).map_err(fail)?,
            },
            "meteora_damm_v1" => {
                let layout = decode_meteora_damm_v1(bytes).map_err(fail)?;
                let vault = |key: &Pubkey| {
                    decode_damm_v1_vault(data(accounts, key).ok_or("vault missing")?)
                        .map_err(|e| e.to_string())
                };
                let a = vault(&layout.a_vault)?;
                let b = vault(&layout.b_vault)?;
                BootLayout::MeteoraDammV1 {
                    layout,
                    a_lp_mint: a.lp_mint,
                    b_lp_mint: b.lp_mint,
                    a_token_vault: a.token_vault,
                    b_token_vault: b.token_vault,
                }
            }
            "pump_amm" => BootLayout::PumpSwap {
                layout: decode_pump_swap(bytes).map_err(fail)?,
            },
            other => return Err(format!("no builder for {other}")),
        };
        Ok(Self { layout, arrays })
    }

    /// The arrays a builder passes: every existing one from the current
    /// position onward in the swap's direction, as the program walks them.
    fn directional(&self, current: i64, down: bool) -> Vec<(i64, Pubkey)> {
        let mut arrays: Vec<(i64, Pubkey)> = self
            .arrays
            .iter()
            .copied()
            .filter(|(start, _)| {
                if down {
                    *start <= current
                } else {
                    *start >= current
                }
            })
            .collect();
        arrays.sort_unstable_by_key(|(start, _)| *start);
        if down {
            arrays.reverse();
        }
        arrays.truncate(MAX_ARRAYS);
        arrays
    }

    pub fn exec(&self, input_mint: &Pubkey) -> Exec {
        let (mint_a, _) = self.layout.mints();
        let a_to_b = *input_mint == mint_a;
        let mut state = HopExecState::none();
        let arrays = match &self.layout {
            BootLayout::RaydiumClmm { layout } => {
                let span = i64::from(layout.tick_spacing)
                    * i64::try_from(RAYDIUM_CLMM_TICKS_PER_ARRAY).expect("ticks per array");
                let current = i64::from(layout.tick_current).div_euclid(span) * span;
                let window = self.directional(current, a_to_b);
                state.clmm_tick_array_starts = window
                    .iter()
                    .map(|(start, _)| i32::try_from(*start).expect("tick array start"))
                    .collect();
                Some(window.len())
            }
            BootLayout::MeteoraDlmm { layout } => {
                let window = self.directional(bin_array_index(layout.active_id), a_to_b);
                state.dlmm_bin_array_indices = window.iter().map(|(index, _)| *index).collect();
                state.dlmm_bin_array_pubkeys = window.iter().map(|(_, key)| *key).collect();
                Some(window.len())
            }
            BootLayout::Whirlpool { layout } => {
                state.whirlpool_tick_current = Some(layout.tick_current);
                Some(WHIRLPOOL_SWAP_TICK_ARRAYS)
            }
            BootLayout::MeteoraDammV2 { layout } => {
                state.damm_v2_layout = Some(layout.clone());
                None
            }
            _ => None,
        };
        Exec {
            state,
            arrays: arrays.map(|n| u8::try_from(n).expect("array count")),
        }
    }
}
