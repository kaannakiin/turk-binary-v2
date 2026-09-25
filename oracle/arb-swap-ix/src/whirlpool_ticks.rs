pub const WHIRLPOOL_SWAP_TICK_ARRAYS: usize = 3;
pub const WHIRLPOOL_TICK_ARRAY_SIZE: i32 = 88;
pub const WHIRLPOOL_MIN_TICK_INDEX: i32 = -443_636;
pub const WHIRLPOOL_MAX_TICK_INDEX: i32 = 443_636;

pub fn whirlpool_swap_tick_array_starts(
    tick_current_index: i32,
    tick_spacing: u16,
    a_to_b: bool,
) -> [Option<i32>; WHIRLPOOL_SWAP_TICK_ARRAYS] {
    let ticks_in_array = WHIRLPOOL_TICK_ARRAY_SIZE * i32::from(tick_spacing);
    let base = tick_current_index.div_euclid(ticks_in_array) * ticks_in_array;
    let offsets = if a_to_b {
        [0, -1, -2]
    } else if tick_current_index + i32::from(tick_spacing) >= base + ticks_in_array {
        [1, 2, 3]
    } else {
        [0, 1, 2]
    };
    offsets.map(|offset| {
        let start = base + offset * ticks_in_array;
        is_valid_start_tick(start, ticks_in_array).then_some(start)
    })
}

fn is_valid_start_tick(tick_index: i32, ticks_in_array: i32) -> bool {
    if !(WHIRLPOOL_MIN_TICK_INDEX..=WHIRLPOOL_MAX_TICK_INDEX).contains(&tick_index) {
        if tick_index > WHIRLPOOL_MIN_TICK_INDEX {
            return false;
        }
        let min_array_start =
            WHIRLPOOL_MIN_TICK_INDEX - (WHIRLPOOL_MIN_TICK_INDEX % ticks_in_array + ticks_in_array);
        return tick_index == min_array_start;
    }
    tick_index % ticks_in_array == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(a_to_b: bool, tick_spacing: u16, tick: i32) -> Vec<i32> {
        whirlpool_swap_tick_array_starts(tick, tick_spacing, a_to_b)
            .into_iter()
            .flatten()
            .collect()
    }

    #[test]
    fn matches_the_vendored_get_start_tick_indexes_table_at_tick_spacing_1() {
        assert_eq!(starts(true, 1, 0), [0, -88, -176]);
        assert_eq!(starts(true, 1, -1), [-88, -176, -264]);
        assert_eq!(starts(true, 1, -443_608), [-443_608, -443_696]);
        assert_eq!(starts(true, 1, -443_635), [-443_696]);
        assert_eq!(starts(false, 1, 86), [0, 88, 176]);
        assert_eq!(starts(false, 1, 87), [88, 176, 264]);
        assert_eq!(starts(false, 1, 443_600), [443_520, 443_608]);
        assert_eq!(starts(false, 1, 443_608), [443_608]);
    }

    #[test]
    fn matches_the_vendored_get_start_tick_indexes_table_at_tick_spacing_64() {
        assert_eq!(starts(true, 64, 0), [0, -5632, -11_264]);
        assert_eq!(starts(true, 64, -64), [-5632, -11_264, -16_896]);
        assert_eq!(starts(true, 64, -439_296), [-439_296, -444_928]);
        assert_eq!(starts(true, 64, -443_635), [-444_928]);
        assert_eq!(starts(false, 64, 5567), [0, 5632, 11_264]);
        assert_eq!(starts(false, 64, 5568), [5632, 11_264, 16_896]);
        assert_eq!(starts(false, 64, 439_200), [433_664, 439_296]);
        assert_eq!(starts(false, 64, 443_608), [439_296]);
    }

    #[test]
    fn matches_the_vendored_get_start_tick_indexes_table_at_tick_spacing_32768() {
        assert_eq!(starts(true, 32_768, 0), [0, -2_883_584]);
        assert_eq!(starts(true, 32_768, -1), [-2_883_584]);
        assert_eq!(starts(true, 32_768, 443_635), [0, -2_883_584]);
        assert_eq!(starts(true, 32_768, -443_635), [-2_883_584]);
        assert_eq!(starts(false, 32_768, -32_769), [-2_883_584, 0]);
        assert_eq!(starts(false, 32_768, -32_768), [0]);
        assert_eq!(starts(false, 32_768, -443_635), [-2_883_584, 0]);
        assert_eq!(starts(false, 32_768, 443_608), [0]);
    }

    #[test]
    fn an_invalid_start_keeps_its_slot_as_none_instead_of_shifting_later_entries() {
        assert_eq!(
            whirlpool_swap_tick_array_starts(-443_608, 1, true),
            [Some(-443_608), Some(-443_696), None]
        );
        assert_eq!(
            whirlpool_swap_tick_array_starts(-32_768, 32_768, false),
            [Some(0), None, None]
        );
    }

    #[test]
    fn the_three_starts_always_share_one_stride_in_every_branch() {
        for (a_to_b, tick) in [(true, 0), (false, 5567), (false, 5568)] {
            let [Some(s0), Some(s1), Some(s2)] = whirlpool_swap_tick_array_starts(tick, 64, a_to_b)
            else {
                panic!("interior ticks yield three starts");
            };
            assert_eq!(s1 - s0, s2 - s1);
            assert_eq!((s1 - s0).abs(), 5632);
        }
    }
}
