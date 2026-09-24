use domain::{DexKind, Pubkey};

use crate::spec::{DexSpec, Discovery};

pub static WHIRLPOOL: DexSpec = DexSpec {
    kind: DexKind::OrcaWhirlpool,
    // src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/lib.rs
    program_id: Pubkey::from_str_const("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"),
    // sha256("account:Whirlpool")[..8]
    // src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/whirlpool.rs
    discriminator: Some([0x3f, 0x95, 0xd1, 0x0c, 0xe1, 0x80, 0x63, 0x09]),
    // Whirlpool::LEN
    // src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/whirlpool.rs
    data_size: Some(653),
    // token_mint_a, token_mint_b
    // src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 programs/whirlpool/src/state/whirlpool.rs
    mint_offsets: Some((101, 181)),
    discovery: Discovery::ProgramAccounts,
    verified: true,
};
