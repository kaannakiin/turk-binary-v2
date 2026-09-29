use router_wire::Hop;

use super::{AUTHORITY, PROGRAM_ID, build, window_len};
use crate::adapters::HopInput;
use crate::token_account::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::{HopAccountView, RouterError};

const USER: [u8; 32] = [9; 32];
const STRANGER: [u8; 32] = [99; 32];
const HOP: Hop = Hop {
    kind: 0,
    hook_a: 0,
    hook_b: 0,
    tail: 0,
    min_out: 0,
};

struct Window {
    keys: Vec<[u8; 32]>,
    owners: Vec<[u8; 32]>,
    signers: Vec<bool>,
    data: Vec<Vec<u8>>,
}

impl Window {
    fn valid() -> Self {
        let mut user_token = vec![0; 165];
        user_token[32..64].copy_from_slice(&USER);
        Self {
            keys: vec![
                PROGRAM_ID,
                TOKEN_PROGRAM_ID,
                [2; 32],
                AUTHORITY,
                [4; 32],
                [5; 32],
                [6; 32],
                [7; 32],
                USER,
            ],
            owners: vec![
                [0; 32],
                [0; 32],
                PROGRAM_ID,
                [0; 32],
                TOKEN_PROGRAM_ID,
                TOKEN_PROGRAM_ID,
                TOKEN_PROGRAM_ID,
                TOKEN_PROGRAM_ID,
                [0; 32],
            ],
            signers: vec![false, false, false, false, false, false, false, false, true],
            data: vec![
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                user_token.clone(),
                user_token,
                vec![],
            ],
        }
    }

    fn build(&self) -> Result<crate::BuiltHop, RouterError> {
        let views: Vec<_> = (0..self.keys.len())
            .map(|i| HopAccountView {
                key: &self.keys[i],
                owner: &self.owners[i],
                is_signer: self.signers[i],
                is_writable: false,
                data: &self.data[i],
            })
            .collect();
        build(&HopInput {
            window: &views,
            amount_in: 1,
            min_out: 0,
            user: &USER,
            source_ata: &self.keys[1],
        })
    }
}

#[test]
fn wrong_program_owner_user_and_token_program_are_refused() {
    type Tamper = fn(&mut Window);
    let cases: [(&str, Tamper); 9] = [
        ("program", |w| w.keys[0] = STRANGER),
        ("pool owner", |w| w.owners[2] = STRANGER),
        ("user", |w| w.keys[8] = STRANGER),
        ("unsigned user", |w| w.signers[8] = false),
        ("source wallet", |w| {
            w.data[6][32..64].copy_from_slice(&STRANGER);
        }),
        ("destination wallet", |w| {
            w.data[7][32..64].copy_from_slice(&STRANGER);
        }),
        ("source Token-2022", |w| w.owners[6] = TOKEN_2022_PROGRAM_ID),
        ("destination Token-2022", |w| {
            w.owners[7] = TOKEN_2022_PROGRAM_ID;
        }),
        ("vault Token-2022", |w| w.owners[4] = TOKEN_2022_PROGRAM_ID),
    ];
    assert!(Window::valid().build().is_ok());
    for (name, mutate) in cases {
        let mut window = Window::valid();
        mutate(&mut window);
        assert_eq!(window.build().err(), Some(RouterError::BadWindow), "{name}");
    }
    let mut short = Window::valid();
    short.keys.pop();
    short.owners.pop();
    short.signers.pop();
    short.data.pop();
    assert_eq!(short.build().err(), Some(RouterError::BadWindow));
}

#[test]
fn hook_and_tail_counts_are_refused_for_legacy_token_v2_swap() {
    assert_eq!(window_len(HOP), Ok(9));
    for hop in [
        Hop { hook_a: 1, ..HOP },
        Hop { hook_b: 1, ..HOP },
        Hop { tail: 1, ..HOP },
    ] {
        assert_eq!(window_len(hop), Err(RouterError::BadWindow));
    }
}
