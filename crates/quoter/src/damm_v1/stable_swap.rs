//! The `StableSwap` invariant DAMM v1's stable curve solves, ported from the
//! crate the program links. Only the pieces a swap uses; `U192` keeps the
//! program's overflow points, so a trade that overflows it declines.

#![expect(
    clippy::similar_names,
    clippy::many_single_char_names,
    reason = "names kept from the ported source"
)]

mod bn {
    #![expect(
        clippy::manual_div_ceil,
        clippy::assign_op_pattern,
        reason = "construct_uint! expansion"
    )]

    // src: mercurial-finance/stable-swap@140c2e0d366765d49edc9a175ed12b1ad10c3b66 stable-swap-math/src/bn.rs (U192)
    uint::construct_uint! {
        pub(crate) struct U192(3);
    }
}

pub(super) use bn::U192;

impl U192 {
    fn to_u128(self) -> Option<u128> {
        self.try_into().ok()
    }
}

// src: mercurial-finance/stable-swap@140c2e0d366765d49edc9a175ed12b1ad10c3b66 stable-swap-math/src/curve.rs (N_COINS)
const N_COINS: u8 = 2;

/// DAMM v1 builds the calculator with the pool's `amp` as both the initial
/// and the target factor and no ramp, so the amplification is `amp`.
// src: MeteoraAg/damm-v1-sdk@02c66a3c13ebabdf71eb29d87996aaa7a06a7c29 dynamic-amm-quote/src/math/stable_swap.rs (From<&StableSwap> for SaberStableSwap: new(amp, amp, 0, 0, 0))
#[derive(Clone, Copy, Debug)]
pub(super) struct StableSwap {
    amp_factor: u64,
}

// src: mercurial-finance/stable-swap@140c2e0d366765d49edc9a175ed12b1ad10c3b66 stable-swap-math/src/curve.rs (compute_next_d2, compute_d2, compute_y_raw2, compute_y2)
impl StableSwap {
    pub(super) const fn new(amp: u64) -> Self {
        Self { amp_factor: amp }
    }

    fn compute_next_d2(self, d_init: U192, d_prod: U192, sum_x: u128) -> Option<U192> {
        let ann = self.amp_factor.checked_mul(N_COINS.into())?;
        let leverage = sum_x.checked_mul(ann.into())?;
        let numerator = d_init.checked_mul(
            d_prod
                .checked_mul(N_COINS.into())?
                .checked_add(leverage.into())?,
        )?;
        let denominator = d_init
            .checked_mul(ann.checked_sub(1)?.into())?
            .checked_add(d_prod.checked_mul((N_COINS.checked_add(1)?).into())?)?;
        numerator.checked_div(denominator)
    }

    pub(super) fn compute_d2(self, amount_a: u128, amount_b: u128) -> Option<U192> {
        let sum_x = amount_a.checked_add(amount_b)?;
        if sum_x == 0 {
            return Some(0.into());
        }
        let amount_a_times_coins = amount_a.checked_mul(N_COINS.into())?;
        let amount_b_times_coins = amount_b.checked_mul(N_COINS.into())?;
        let mut d: U192 = sum_x.into();
        for _ in 0..256 {
            let mut d_prod = d;
            d_prod = d_prod
                .checked_mul(d)?
                .checked_div(amount_a_times_coins.into())?;
            d_prod = d_prod
                .checked_mul(d)?
                .checked_div(amount_b_times_coins.into())?;
            let d_prev = d;
            d = self.compute_next_d2(d, d_prod, sum_x)?;
            if d > d_prev {
                if d.checked_sub(d_prev)? <= 1.into() {
                    break;
                }
            } else if d_prev.checked_sub(d)? <= 1.into() {
                break;
            }
        }
        Some(d)
    }

    fn compute_y_raw2(self, x: u128, d: U192) -> Option<U192> {
        let ann = self.amp_factor.checked_mul(N_COINS.into())?;
        let mut c = d
            .checked_mul(d)?
            .checked_div(x.checked_mul(N_COINS.into())?.into())?;
        c = c
            .checked_mul(d)?
            .checked_div(ann.checked_mul(N_COINS.into())?.into())?;
        let b = d.checked_div(ann.into())?.checked_add(x.into())?;
        let mut y = d;
        for _ in 0..256 {
            let y_prev = y;
            let y_numerator = y.checked_pow(2.into())?.checked_add(c)?;
            let y_denominator = y.checked_mul(2.into())?.checked_add(b)?.checked_sub(d)?;
            y = y_numerator.checked_div(y_denominator)?;
            if y > y_prev {
                if y.checked_sub(y_prev)? <= 1.into() {
                    break;
                }
            } else if y_prev.checked_sub(y)? <= 1.into() {
                break;
            }
        }
        Some(y)
    }

    pub(super) fn compute_y2(self, x: u128, d: U192) -> Option<u128> {
        self.compute_y_raw2(x, d)?.to_u128()
    }
}
