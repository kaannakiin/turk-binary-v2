use router_wire::Hop;

use super::{MEMO_PROGRAM_ID, PROGRAM_ID, build, window_len};
use crate::adapters::HopInput;
use crate::token_account::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::{HopAccountView, RouterError};

const USER: [u8; 32] = [1; 32];
const HOP: Hop = Hop {
    kind: 3,
    hook_a: 0,
    hook_b: 0,
    tail: 0,
    min_out: 7,
};

struct Window {
    keys: [[u8; 32]; 16],
    owners: [[u8; 32]; 16],
    writable: [bool; 16],
    data: [Vec<u8>; 16],
}

impl Window {
    fn mixed_token_programs() -> Self {
        let mut keys = core::array::from_fn(|i| [u8::try_from(i).expect("index"); 32]);
        keys[0] = PROGRAM_ID;
        keys[1] = TOKEN_2022_PROGRAM_ID;
        keys[2] = TOKEN_PROGRAM_ID;
        keys[3] = MEMO_PROGRAM_ID;
        keys[4] = USER;
        let mut owners = [[0; 32]; 16];
        owners[5] = PROGRAM_ID;
        owners[6] = TOKEN_2022_PROGRAM_ID;
        owners[7] = TOKEN_PROGRAM_ID;
        owners[8] = TOKEN_2022_PROGRAM_ID;
        owners[9] = TOKEN_2022_PROGRAM_ID;
        owners[10] = TOKEN_PROGRAM_ID;
        owners[11] = TOKEN_PROGRAM_ID;
        let mut data: [Vec<u8>; 16] = core::array::from_fn(|_| Vec::new());
        data[5] = vec![0; 653];
        data[5][..8].copy_from_slice(&[0x3f, 0x95, 0xd1, 0x0c, 0xe1, 0x80, 0x63, 0x09]);
        data[5][41..43].copy_from_slice(&1u16.to_le_bytes());
        for (range, index) in [(101..133, 6), (133..165, 9), (181..213, 7), (213..245, 11)] {
            data[5][range].copy_from_slice(&keys[index]);
        }
        for (account, mint, owner) in [
            (8, 6, USER),
            (9, 6, [20; 32]),
            (10, 7, USER),
            (11, 7, [20; 32]),
        ] {
            data[account] = vec![0; 165];
            data[account][..32].copy_from_slice(&keys[mint]);
            data[account][32..64].copy_from_slice(&owner);
        }
        let mut writable = [false; 16];
        for index in [5, 8, 9, 10, 11, 12, 13, 14, 15] {
            writable[index] = true;
        }
        Self {
            keys,
            owners,
            writable,
            data,
        }
    }

    fn build(&self, source: usize, hop: Hop) -> Result<crate::BuiltHop, RouterError> {
        let views: Vec<_> = (0..self.keys.len())
            .map(|index| HopAccountView {
                key: &self.keys[index],
                owner: &self.owners[index],
                is_signer: index == 4,
                is_writable: self.writable[index],
                data: &self.data[index],
            })
            .collect();
        build(
            hop,
            &HopInput {
                window: &views,
                amount_in: 11,
                min_out: 7,
                user: &USER,
                source_ata: &self.keys[source],
            },
        )
    }
}

#[test]
fn swap_v2_exact_input_uses_program_account_order_and_direction() {
    // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
    // programs/whirlpool/src/instructions/v2/swap.rs (six args, 15 accounts).
    let window = Window::mixed_token_programs();
    assert_eq!(window_len(HOP), Ok(16));
    for (source, in_index, out_index, a_to_b, limit) in [
        (8, 8, 10, 1, 4_295_048_016u128),
        (10, 10, 8, 0, 79_226_673_515_401_279_992_447_579_055u128),
    ] {
        let built = window.build(source, HOP).expect("source-valid window");
        let mut expected = vec![0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62];
        expected.extend_from_slice(&11u64.to_le_bytes());
        expected.extend_from_slice(&7u64.to_le_bytes());
        expected.extend_from_slice(&limit.to_le_bytes());
        expected.extend_from_slice(&[1, a_to_b, 0]);
        assert_eq!(built.ix.data, expected);
        assert_eq!(built.in_ata_index, in_index);
        assert_eq!(built.out_ata_index, out_index);
        assert_eq!(built.ix.metas.len(), 15);
        assert_eq!(built.ix.metas[0].key, TOKEN_2022_PROGRAM_ID);
        assert_eq!(built.ix.metas[1].key, TOKEN_PROGRAM_ID);
        assert_eq!(built.ix.metas[14].key, window.keys[15]);
    }
}

#[test]
fn wrong_pool_mint_vault_and_token_program_are_refused() {
    for tamper in [
        |w: &mut Window| w.data[5][101] ^= 1,
        |w: &mut Window| w.data[5][133] ^= 1,
        |w: &mut Window| w.keys[1][0] ^= 1,
        |w: &mut Window| w.owners[8] = TOKEN_PROGRAM_ID,
    ] {
        let mut window = Window::mixed_token_programs();
        tamper(&mut window);
        assert_eq!(
            window.build(8, HOP).map(|_| ()),
            Err(RouterError::BadWindow)
        );
    }
}

#[test]
fn unsupported_hooks_and_invalid_source_are_refused() {
    let window = Window::mixed_token_programs();
    assert_eq!(
        window.build(12, HOP).map(|_| ()),
        Err(RouterError::HopContinuityViolation)
    );
    assert_eq!(
        window_len(Hop { hook_a: 1, ..HOP }),
        Err(RouterError::BadWindow)
    );
    assert_eq!(
        window_len(Hop { tail: 3, ..HOP }),
        Err(RouterError::BadWindow)
    );
}

#[test]
fn fixed_and_dynamic_tick_arrays_use_their_distinct_pool_offsets() {
    // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
    // programs/whirlpool/src/state/{fixed_tick_array,dynamic_tick_array}.rs.
    let mut window = Window::mixed_token_programs();
    window.owners[12] = PROGRAM_ID;
    window.data[12] = vec![0; 9_988];
    window.data[12][..8].copy_from_slice(&[0x45, 0x61, 0xbd, 0xbe, 0x6e, 0x07, 0x42, 0xbb]);
    window.data[12][9_956..9_988].copy_from_slice(&window.keys[5]);
    window.owners[13] = PROGRAM_ID;
    window.data[13] = vec![0; 148];
    window.data[13][..8].copy_from_slice(&[0x11, 0xd8, 0xf6, 0x8e, 0xe1, 0xc7, 0xda, 0x38]);
    window.data[13][8..12].copy_from_slice(&(-88i32).to_le_bytes());
    window.data[13][12..44].copy_from_slice(&window.keys[5]);
    assert!(window.build(8, HOP).is_ok());
    window.data[12][9_956] ^= 1;
    assert_eq!(
        window.build(8, HOP).map(|_| ()),
        Err(RouterError::BadWindow)
    );
}

#[test]
fn named_tick_arrays_away_from_the_pool_tick_or_out_of_order_reach_the_program() {
    // src: kaannakiin/whirlpools@536d2dac6c53eb50da09b4534ac5113b5c5c7052
    // programs/whirlpool/src/util/sparse_swap.rs (SparseSwapTickSequenceBuilder::new:
    // tick arrays can be provided in any order; extras are a fallback if the price moves).
    let mut window = Window::mixed_token_programs();
    for (slot, start) in [(12, 176i32), (13, 88), (14, 264)] {
        window.owners[slot] = PROGRAM_ID;
        window.data[slot] = vec![0; 148];
        window.data[slot][..8].copy_from_slice(&[0x11, 0xd8, 0xf6, 0x8e, 0xe1, 0xc7, 0xda, 0x38]);
        window.data[slot][8..12].copy_from_slice(&start.to_le_bytes());
        window.data[slot][12..44].copy_from_slice(&window.keys[5]);
    }
    for source in [8, 10] {
        assert!(window.build(source, HOP).is_ok());
    }
}

#[test]
fn readonly_pool_vault_array_or_oracle_is_refused() {
    for index in [5, 8, 9, 10, 11, 12, 13, 14, 15] {
        let mut window = Window::mixed_token_programs();
        window.writable[index] = false;
        assert_eq!(
            window.build(8, HOP).map(|_| ()),
            Err(RouterError::BadWindow)
        );
    }
}
