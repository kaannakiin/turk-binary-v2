use crate::RouterError;

const ACTUAL_IN_FLOOR_NUM: u128 = 95;
const ACTUAL_IN_FLOOR_DEN: u128 = 100;

pub fn check_route_args(in_amount: u64, min_out: u64, circular: bool) -> Result<(), RouterError> {
    if in_amount == 0 || min_out == 0 {
        return Err(RouterError::BadArgs);
    }
    if circular && min_out <= in_amount {
        return Err(RouterError::CircularRouteNotProfitable);
    }
    Ok(())
}

// A venue that pulls far less than it was offered leaves the rest stranded in
// the intermediate account and desyncs every later hop's economics.
pub fn check_actual_in_band(
    source_before: u64,
    source_after: u64,
    amount_in: u64,
) -> Result<u64, RouterError> {
    let actual_in = source_before
        .checked_sub(source_after)
        .ok_or(RouterError::BalanceRegression)?;
    if actual_in > amount_in {
        return Err(RouterError::ActualInOutOfBand);
    }
    let scaled_actual = u128::from(actual_in)
        .checked_mul(ACTUAL_IN_FLOOR_DEN)
        .ok_or(RouterError::ActualInOutOfBand)?;
    let scaled_floor = u128::from(amount_in)
        .checked_mul(ACTUAL_IN_FLOOR_NUM)
        .ok_or(RouterError::ActualInOutOfBand)?;
    if scaled_actual < scaled_floor {
        return Err(RouterError::ActualInOutOfBand);
    }
    Ok(actual_in)
}

pub fn check_hop_output(out_before: u64, out_after: u64) -> Result<u64, RouterError> {
    let delta = out_after
        .checked_sub(out_before)
        .ok_or(RouterError::BalanceRegression)?;
    if delta == 0 {
        return Err(RouterError::ZeroHopOutput);
    }
    Ok(delta)
}

pub fn check_min_out(amount_out: u64, min_out: u64) -> Result<(), RouterError> {
    if amount_out < min_out {
        return Err(RouterError::SlippageExceeded);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hop_must_consume_between_95_and_100_percent_of_its_input() {
        let cases = [
            ((10_000, 500, 10_000), Ok(9_500)),
            ((10_000, 501, 10_000), Err(RouterError::ActualInOutOfBand)),
            ((10_000, 0, 10_000), Ok(10_000)),
            ((10_000, 0, 9_999), Err(RouterError::ActualInOutOfBand)),
            (
                (10_000, 10_001, 10_000),
                Err(RouterError::BalanceRegression),
            ),
            ((u64::MAX, 0, u64::MAX), Ok(u64::MAX)),
        ];
        for ((before, after, offered), expected) in cases {
            assert_eq!(
                check_actual_in_band(before, after, offered),
                expected,
                "before={before} after={after} offered={offered}"
            );
        }
    }

    #[test]
    fn a_hop_must_strictly_grow_its_output_account() {
        assert_eq!(check_hop_output(5, 12), Ok(7));
        assert_eq!(check_hop_output(5, 5), Err(RouterError::ZeroHopOutput));
        assert_eq!(check_hop_output(5, 4), Err(RouterError::BalanceRegression));
    }

    #[test]
    fn a_circular_route_must_promise_more_than_it_spends() {
        assert_eq!(check_route_args(100, 101, true), Ok(()));
        assert_eq!(
            check_route_args(100, 100, true),
            Err(RouterError::CircularRouteNotProfitable)
        );
        assert_eq!(check_route_args(100, 1, false), Ok(()));
        assert_eq!(check_route_args(0, 1, false), Err(RouterError::BadArgs));
        assert_eq!(check_route_args(1, 0, false), Err(RouterError::BadArgs));
    }

    #[test]
    fn output_below_min_out_is_slippage() {
        assert_eq!(check_min_out(99, 100), Err(RouterError::SlippageExceeded));
        assert_eq!(check_min_out(100, 100), Ok(()));
    }
}
