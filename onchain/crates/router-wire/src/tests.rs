use crate::{CONFIG_LEN, Config, DecodeError, Hop, Route, RouterInstruction};

fn two_hop_route() -> RouterInstruction {
    let hops = [
        Hop {
            kind: 3,
            hook_a: 0,
            hook_b: 0,
            tail: 0,
            min_out: 11,
        },
        Hop {
            kind: 1,
            hook_a: 2,
            hook_b: 0,
            tail: 3,
            min_out: 22,
        },
    ];
    RouterInstruction::Route(Route::new(1_000, 0x0102_0304_0506_0708, &hops).unwrap())
}

#[rustfmt::skip]
const TWO_HOP_ROUTE_BYTES: [u8; 43] = [
    0, 2,
    0xe8, 0x03, 0, 0, 0, 0, 0, 0,
    0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01,
    2,
    3, 0, 0, 0,
    11, 0, 0, 0, 0, 0, 0, 0,
    1, 2, 0, 3,
    22, 0, 0, 0, 0, 0, 0, 0,
];

#[test]
fn every_instruction_matches_the_documented_bytes() {
    let admin = [7u8; 32];
    let cases: [(RouterInstruction, Vec<u8>); 5] = [
        (two_hop_route(), TWO_HOP_ROUTE_BYTES.to_vec()),
        (
            RouterInstruction::Initialize { admin },
            [&[1u8][..], &admin].concat(),
        ),
        (RouterInstruction::SetPaused { paused: true }, vec![2, 1]),
        (RouterInstruction::SetPaused { paused: false }, vec![2, 0]),
        (
            RouterInstruction::SetAdmin { new_admin: admin },
            [&[3u8][..], &admin].concat(),
        ),
    ];
    for (instruction, bytes) in cases {
        assert_eq!(instruction.encode(), bytes, "{instruction:?}");
        assert_eq!(RouterInstruction::decode(&bytes), Ok(instruction));
    }
}

#[test]
fn malformed_instruction_data_is_refused_with_its_reason() {
    let mut wrong_version = TWO_HOP_ROUTE_BYTES;
    wrong_version[1] = 1;
    let mut zero_hops = TWO_HOP_ROUTE_BYTES[..19].to_vec();
    zero_hops[18] = 0;
    let mut five_hops = TWO_HOP_ROUTE_BYTES.to_vec();
    five_hops[18] = 5;
    five_hops.extend_from_slice(&[0; 12]);
    let mut count_says_three = TWO_HOP_ROUTE_BYTES;
    count_says_three[18] = 3;

    let cases: [(&str, Vec<u8>, DecodeError); 10] = [
        ("empty", vec![], DecodeError::Length),
        ("unknown tag", vec![4], DecodeError::UnknownInstruction),
        (
            "route version",
            wrong_version.to_vec(),
            DecodeError::UnsupportedVersion,
        ),
        ("zero hops", zero_hops, DecodeError::HopCount),
        ("five hops", five_hops, DecodeError::HopCount),
        (
            "count past data",
            count_says_three.to_vec(),
            DecodeError::Length,
        ),
        (
            "trailing byte",
            [&TWO_HOP_ROUTE_BYTES[..], &[0]].concat(),
            DecodeError::Length,
        ),
        ("paused not bool", vec![2, 2], DecodeError::InvalidBool),
        ("paused missing", vec![2], DecodeError::Length),
        (
            "short admin",
            [&[1u8][..], &[7; 31]].concat(),
            DecodeError::Length,
        ),
    ];
    for (name, bytes, expected) in cases {
        assert_eq!(RouterInstruction::decode(&bytes), Err(expected), "{name}");
    }
}

#[test]
fn config_matches_the_documented_layout() {
    let config = Config {
        admin: [9u8; 32],
        paused: true,
        bump: 254,
    };
    let mut bytes = [0u8; CONFIG_LEN];
    bytes[0] = 1;
    bytes[1] = 1;
    bytes[2..34].copy_from_slice(&[9u8; 32]);
    bytes[34] = 1;
    bytes[35] = 254;

    assert_eq!(config.encode(), bytes);
    assert_eq!(Config::decode(&bytes), Ok(config));
}

#[test]
fn a_foreign_or_damaged_config_account_is_refused() {
    let good = Config {
        admin: [9u8; 32],
        paused: false,
        bump: 1,
    }
    .encode();
    let mut other_type = good;
    other_type[0] = 2;
    let mut newer = good;
    newer[1] = 2;
    let mut bad_paused = good;
    bad_paused[34] = 7;

    let cases: [(&str, &[u8], DecodeError); 4] = [
        ("short", &good[..CONFIG_LEN - 1], DecodeError::Length),
        ("discriminator", &other_type, DecodeError::Discriminator),
        ("version", &newer, DecodeError::UnsupportedVersion),
        ("paused", &bad_paused, DecodeError::InvalidBool),
    ];
    for (name, bytes, expected) in cases {
        assert_eq!(Config::decode(bytes), Err(expected), "{name}");
    }
}
