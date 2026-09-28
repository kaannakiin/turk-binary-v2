use crate::RouterError;

// src: solana-sdk-ids@3.1.0 src/lib.rs (bpf_loader_upgradeable::ID)
pub const BPF_LOADER_UPGRADEABLE_ID: [u8; 32] = [
    2, 168, 246, 145, 78, 136, 161, 176, 226, 16, 21, 62, 247, 99, 174, 43, 0, 194, 185, 61, 22,
    193, 36, 210, 192, 83, 122, 16, 4, 128, 0, 0,
];

// src: solana-loader-v3-interface@8.0.1 src/state.rs (UpgradeableLoaderState::ProgramData,
// bincode: u32 variant 3, slot u64, Option<Pubkey>; size_of_programdata_metadata() == 45)
const PROGRAM_DATA_TAG: [u8; 4] = [3, 0, 0, 0];
const OPTION_TAG: usize = 12;
const AUTHORITY: core::ops::Range<usize> = 13..45;

pub fn upgrade_authority(program_data: &[u8]) -> Result<Option<[u8; 32]>, RouterError> {
    if program_data.len() < AUTHORITY.end || program_data[..4] != PROGRAM_DATA_TAG {
        return Err(RouterError::BadProgramData);
    }
    match program_data[OPTION_TAG] {
        0 => Ok(None),
        1 => {
            let mut authority = [0u8; 32];
            authority.copy_from_slice(&program_data[AUTHORITY]);
            Ok(Some(authority))
        }
        _ => Err(RouterError::BadProgramData),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // src: mainnet getAccountInfo DMawCQzbgNTmbzaESc7o6pvL1KAeetY8zA7jNpzntHhU (Raydium CPMM's
    // ProgramData) at slot 451384914, dataSlice 0..45.
    #[rustfmt::skip]
    const CPMM_PROGRAM_DATA_HEADER: [u8; 45] = [
        3, 0, 0, 0, 176, 207, 145, 26, 0, 0, 0, 0, 1, 222, 150, 15, 9, 106, 255, 167, 1, 64, 61,
        250, 151, 145, 180, 222, 110, 154, 207, 27, 166, 19, 64, 253, 96, 222, 18, 166, 54, 162,
        194, 252, 64,
    ];

    // src: the same account's jsonParsed `authority`, FytDrVzDybM1TwFQPGb8qaxZR7dBCzNeqT3vtQsceZQK.
    const CPMM_UPGRADE_AUTHORITY: [u8; 32] = [
        222, 150, 15, 9, 106, 255, 167, 1, 64, 61, 250, 151, 145, 180, 222, 110, 154, 207, 27, 166,
        19, 64, 253, 96, 222, 18, 166, 54, 162, 194, 252, 64,
    ];

    #[test]
    fn reads_the_upgrade_authority_of_a_mainnet_program() {
        assert_eq!(
            upgrade_authority(&CPMM_PROGRAM_DATA_HEADER),
            Ok(Some(CPMM_UPGRADE_AUTHORITY))
        );
    }

    #[test]
    fn an_immutable_program_has_no_authority() {
        let mut immutable = CPMM_PROGRAM_DATA_HEADER;
        immutable[OPTION_TAG] = 0;
        assert_eq!(upgrade_authority(&immutable), Ok(None));
    }

    #[test]
    fn anything_but_program_data_is_refused() {
        let mut program_account = CPMM_PROGRAM_DATA_HEADER;
        program_account[0] = 2;
        let mut bad_option = CPMM_PROGRAM_DATA_HEADER;
        bad_option[OPTION_TAG] = 2;
        let cases: [(&str, &[u8]); 3] = [
            ("program account tag", &program_account),
            ("option tag", &bad_option),
            ("short", &CPMM_PROGRAM_DATA_HEADER[..44]),
        ];
        for (name, data) in cases {
            assert_eq!(
                upgrade_authority(data),
                Err(RouterError::BadProgramData),
                "{name}"
            );
        }
    }
}
