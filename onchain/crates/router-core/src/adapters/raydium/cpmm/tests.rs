use router_wire::Hop;

use super::{PROGRAM_ID, build, window_len};
use crate::adapters::HopInput;
use crate::token_account::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::{HopAccountView, RouterError};

// src: mainnet tx 49Gr3dn1wF3QkLzACMVCnnZj9SXRKe3UgWAxdc7oL11fhncYR2Sn7fwqzgpWwRyL72CSSeU29x7cUCb9cnetC2PX
// (slot 451386322), its top-level swap_base_input: accounts with (signer, writable), then data.
const MAINNET_ACCOUNTS: [(&str, bool, bool); 13] = [
    ("3XHtXZ9sdzoQvqjKadyn4JP7kAfQACHpSpGAbbusd3tq", true, true),
    ("GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL", false, false),
    ("CRRS5ieQmBrZjWhcj99JuGrT5tyuWDaGAXLXLFjbAtjQ", false, false),
    ("JChMLUsXQMqZ2n6YsKZ4XxZ2xpSCz4YfZPeS5DPTsmAY", false, true),
    ("HKvTnBHkG2VXexRn6CJoKBGy9Yt95vzujUkhosf3BdjG", false, true),
    ("4jrLyVVAXCauJwF8EWFa6UWEjidZQDcuzCctSiza26Vv", false, true),
    ("BPG37RBkvnEy58S1RE2pyyH6cxHAXkEULSoNzDiUrEj7", false, true),
    ("22BChCPw2CyjpKNh3D3Hzd6CuP2YcmWL7GTBtaRYZYAG", false, true),
    ("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb", false, false),
    ("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", false, false),
    ("DsEGN7EuSE5MDhs28iBUkqGE6ptTJ3Wx8c9M8KbUWtuN", false, false),
    ("DoGEV7LASBkQbibMc5k5vKnTZoMg423GpJ5QtJEGfm7R", false, false),
    ("Euj3kuVVtK112iUZnL9D2YzNmAbMUQBSTqoAxAuiX3WT", false, true),
];
const MAINNET_DATA: [u8; 24] = [
    143, 190, 90, 218, 196, 30, 51, 222, 51, 210, 10, 231, 17, 0, 0, 0, 25, 57, 69, 114, 0, 0, 0, 0,
];
const MAINNET_AMOUNT_IN: u64 = 76_890_690_099;

const CPMM_HOP: Hop = Hop {
    kind: 2,
    hook_a: 0,
    hook_b: 0,
    tail: 0,
    min_out: 0,
};

fn key(base58: &str) -> [u8; 32] {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut bytes = [0u8; 32];
    for c in base58.bytes() {
        let mut carry = u32::try_from(ALPHABET.iter().position(|&a| a == c).unwrap()).unwrap();
        for byte in bytes.iter_mut().rev() {
            carry = u32::from(*byte)
                .checked_mul(58)
                .and_then(|scaled| scaled.checked_add(carry))
                .unwrap();
            *byte = u8::try_from(carry & 0xff).unwrap();
            carry >>= 8;
        }
        assert_eq!(carry, 0, "{base58} overflows 32 bytes");
    }
    bytes
}

struct Window {
    keys: Vec<[u8; 32]>,
    owners: Vec<[u8; 32]>,
    flags: Vec<(bool, bool)>,
    data: Vec<Vec<u8>>,
}

impl Window {
    fn mainnet() -> Self {
        let payer = key(MAINNET_ACCOUNTS[0].0);
        let mut token_account = vec![0u8; 165];
        token_account[32..64].copy_from_slice(&payer);

        let mut window = Self {
            keys: vec![PROGRAM_ID],
            owners: vec![[0; 32]],
            flags: vec![(false, false)],
            data: vec![vec![]],
        };
        for (index, &(address, signer, writable)) in MAINNET_ACCOUNTS.iter().enumerate() {
            let (owner, data) = match index {
                3 => (PROGRAM_ID, vec![]),
                4 => (TOKEN_2022_PROGRAM_ID, token_account.clone()),
                5 => (TOKEN_PROGRAM_ID, token_account.clone()),
                _ => ([0; 32], vec![]),
            };
            window.keys.push(key(address));
            window.owners.push(owner);
            window.flags.push((signer, writable));
            window.data.push(data);
        }
        window
    }

    fn views(&self) -> Vec<HopAccountView<'_>> {
        (0..self.keys.len())
            .map(|i| HopAccountView {
                key: &self.keys[i],
                owner: &self.owners[i],
                is_signer: self.flags[i].0,
                is_writable: self.flags[i].1,
                data: &self.data[i],
            })
            .collect()
    }

    fn build(&self) -> Result<crate::BuiltHop, RouterError> {
        let views = self.views();
        build(&HopInput {
            window: &views,
            amount_in: MAINNET_AMOUNT_IN,
            min_out: 0,
            user: &key(MAINNET_ACCOUNTS[0].0),
        })
    }
}

#[test]
fn builds_the_instruction_mainnet_executed() {
    let built = Window::mainnet().build().unwrap();

    let metas: Vec<([u8; 32], bool, bool)> = built
        .ix
        .metas
        .iter()
        .map(|meta| (meta.key, meta.is_signer, meta.is_writable))
        .collect();
    let expected: Vec<([u8; 32], bool, bool)> = MAINNET_ACCOUNTS
        .iter()
        .map(|&(address, signer, writable)| (key(address), signer, writable))
        .collect();
    assert_eq!(metas, expected);
    assert_eq!(built.ix.program_id, PROGRAM_ID);
    assert_eq!(built.ix.data[..16], MAINNET_DATA[..16]);
    assert_eq!(
        built.ix.data[16..],
        [0u8; 8],
        "per-hop min_out is the route's job"
    );
}

#[test]
fn reads_balances_off_the_user_token_accounts_mainnet_used() {
    let window = Window::mainnet();
    let built = window.build().unwrap();
    assert_eq!(window.keys[built.in_ata_index], key(MAINNET_ACCOUNTS[4].0));
    assert_eq!(window.keys[built.out_ata_index], key(MAINNET_ACCOUNTS[5].0));
}

type Tamper = fn(&mut Window, [u8; 32]);

#[test]
fn a_window_that_is_not_the_users_cpmm_swap_is_refused() {
    let stranger = [7u8; 32];
    let cases: [(&str, Tamper); 5] = [
        ("other program in slot 0", |w, s| w.keys[0] = s),
        ("pool not owned by CPMM", |w, s| w.owners[4] = s),
        ("payer is not the user", |w, s| w.keys[1] = s),
        ("input account of another wallet", |w, s| {
            w.data[5][32..64].copy_from_slice(&s);
        }),
        ("output account of another wallet", |w, s| {
            w.data[6][32..64].copy_from_slice(&s);
        }),
    ];
    for (name, mutate) in cases {
        let mut window = Window::mainnet();
        mutate(&mut window, stranger);
        assert_eq!(window.build().err(), Some(RouterError::BadWindow), "{name}");
    }
}

#[test]
fn hook_or_tail_accounts_are_refused() {
    for hop in [
        Hop {
            hook_a: 1,
            ..CPMM_HOP
        },
        Hop {
            hook_b: 1,
            ..CPMM_HOP
        },
        Hop {
            tail: 1,
            ..CPMM_HOP
        },
    ] {
        assert_eq!(window_len(hop), Err(RouterError::BadWindow), "{hop:?}");
    }
    assert_eq!(window_len(CPMM_HOP), Ok(14));
}
