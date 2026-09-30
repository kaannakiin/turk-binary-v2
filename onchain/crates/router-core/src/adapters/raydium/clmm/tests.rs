use router_wire::Hop;

use super::{PROGRAM_ID, build, window_len};
use crate::adapters::HopInput;
use crate::token_account::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::{HopAccountView, RouterError};

const USER: [u8; 32] = [1; 32];
const CLMM_HOP: Hop = Hop {
    kind: 1,
    hook_a: 0,
    hook_b: 0,
    tail: 0x82,
    min_out: 7,
};

struct Window {
    keys: Vec<[u8; 32]>,
    owners: Vec<[u8; 32]>,
    writable: Vec<bool>,
    data: Vec<Vec<u8>>,
}

impl Window {
    fn extension_path() -> Self {
        let mut keys: Vec<[u8; 32]> = (0..17).map(|index| [index; 32]).collect();
        keys[0] = PROGRAM_ID;
        keys[1] = USER;
        keys[9] = TOKEN_PROGRAM_ID;
        keys[10] = TOKEN_2022_PROGRAM_ID;
        let mut owners = vec![[0; 32]; 17];
        for index in [2, 3, 8, 14, 15, 16] {
            owners[index] = PROGRAM_ID;
        }
        for index in [4, 5, 6, 7, 12, 13] {
            owners[index] = TOKEN_PROGRAM_ID;
        }
        let mut data = vec![vec![]; 17];
        data[3] = vec![0; 233];
        for (range, key) in [
            (9..41, keys[2]),
            (73..105, keys[12]),
            (105..137, keys[13]),
            (137..169, keys[6]),
            (169..201, keys[7]),
            (201..233, keys[8]),
        ] {
            data[3][range].copy_from_slice(&key);
        }
        for (account, mint, owner) in [
            (4, 12, USER),
            (5, 13, USER),
            (6, 12, [20; 32]),
            (7, 13, [20; 32]),
        ] {
            data[account] = vec![0; 165];
            data[account][..32].copy_from_slice(&keys[mint]);
            data[account][32..64].copy_from_slice(&owner);
        }
        data[14] = vec![0; 1832];
        data[14][8..40].copy_from_slice(&keys[3]);
        for (index, start) in [(15, 100i32), (16, 40i32)] {
            data[index] = vec![0; 10240];
            data[index][8..40].copy_from_slice(&keys[3]);
            data[index][40..44].copy_from_slice(&start.to_le_bytes());
        }
        let writable = (0..17)
            .map(|index| matches!(index, 3..=8 | 14..=16))
            .collect();
        Self {
            keys,
            owners,
            writable,
            data,
        }
    }

    fn build(&self, hop: Hop) -> Result<crate::BuiltHop, RouterError> {
        let views: Vec<_> = (0..self.keys.len())
            .map(|index| HopAccountView {
                key: &self.keys[index],
                owner: &self.owners[index],
                is_signer: index == 1,
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
                source_ata: &self.keys[4],
            },
        )
    }
}

#[test]
fn bitmap_extension_precedes_ordered_tick_arrays_in_exact_in_cpi() {
    let window = Window::extension_path();
    assert_eq!(window_len(CLMM_HOP), Ok(17));
    let built = window
        .build(CLMM_HOP)
        .expect("source-valid extension window");
    assert_eq!(built.ix.program_id, PROGRAM_ID);
    assert_eq!(built.ix.metas.len(), 16);
    assert_eq!(built.ix.metas[13].key, window.keys[14]);
    assert_eq!(built.ix.metas[14].key, window.keys[15]);
    assert_eq!(built.ix.metas[15].key, window.keys[16]);
    assert_eq!(
        &built.ix.data[..8],
        &[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]
    );
    assert_eq!(&built.ix.data[8..16], &11u64.to_le_bytes());
    assert_eq!(&built.ix.data[16..24], &7u64.to_le_bytes());
    assert_eq!(built.ix.data[40], 1);
}

#[test]
fn extension_or_tick_array_from_another_pool_is_refused() {
    for index in [14, 15, 16] {
        let mut window = Window::extension_path();
        window.data[index][8..40].fill(99);
        assert!(
            matches!(window.build(CLMM_HOP), Err(RouterError::BadWindow)),
            "account {index}"
        );
    }
    let mut reversed = Window::extension_path();
    reversed.data.swap(15, 16);
    assert!(matches!(
        reversed.build(CLMM_HOP),
        Err(RouterError::BadWindow)
    ));
    let mut missing = Window::extension_path();
    missing.keys.remove(14);
    missing.owners.remove(14);
    missing.writable.remove(14);
    missing.data.remove(14);
    assert!(matches!(
        missing.build(CLMM_HOP),
        Err(RouterError::BadWindow)
    ));
}
