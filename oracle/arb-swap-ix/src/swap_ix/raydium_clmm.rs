use solana_instruction::Instruction;

use crate::layout::raydium_clmm::RaydiumClmmLayout;
use crate::registry::RAYDIUM_CLMM_PROGRAM_ID;

use super::clmm_common::build_clmm_swap_v2;
use super::{SwapHopContext, SwapIxBuilder, SwapIxError, WindowVec};

#[derive(Debug, Clone)]
pub struct RaydiumClmmSwapIx {
    pub layout: RaydiumClmmLayout,
    pub tick_array_starts: WindowVec<i32>,
}

impl SwapIxBuilder for RaydiumClmmSwapIx {
    fn build(&self, ctx: &SwapHopContext<'_>) -> Result<Instruction, SwapIxError> {
        build_clmm_swap_v2(
            &self.layout,
            &self.tick_array_starts,
            ctx,
            RAYDIUM_CLMM_PROGRAM_ID,
        )
    }
}
