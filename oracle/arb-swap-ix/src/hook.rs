use solana_instruction::AccountMeta;
use solana_pubkey::Pubkey;

const EXTRA_ACCOUNT_METAS_SEED: &[u8] = b"extra-account-metas";

pub const EXECUTE_DISCRIMINATOR: [u8; 8] = [105, 37, 101, 197, 75, 251, 102, 26];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraMeta {
    pub discriminator: u8,
    pub address_config: [u8; 32],
    pub is_signer: bool,
    pub is_writable: bool,
    pub address: HookAddress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSeed {
    Literal(Vec<u8>),
    InstructionData {
        index: u8,
        length: u8,
    },
    AccountKey {
        index: u8,
    },
    AccountData {
        account_index: u8,
        data_index: u8,
        length: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookAddress {
    Literal([u8; 32]),
    Pda {
        program_index: Option<u8>,
        seeds: Vec<HookSeed>,
    },
    InstructionData {
        index: u8,
    },
    AccountData {
        account_index: u8,
        data_index: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookDefinition {
    pub metas: Vec<ExtraMeta>,
    pub requires_transfer_context: bool,
}

fn execute_record(mut data: &[u8]) -> Option<&[u8]> {
    let mut found = None;
    while !data.is_empty() {
        if data.iter().all(|b| *b == 0) {
            break;
        }
        let len = u32::from_le_bytes(data.get(8..12)?.try_into().ok()?) as usize;
        let end = 12usize.checked_add(len)?;
        let record = data.get(..end)?;
        if record[..8] == EXECUTE_DISCRIMINATOR {
            if found.is_some() {
                return None;
            }
            let count = u32::from_le_bytes(record.get(12..16)?.try_into().ok()?) as usize;
            if len != 4usize.checked_add(count.checked_mul(META_LEN)?)? {
                return None;
            }
            found = Some(record);
        }
        data = &data[end..];
    }
    found
}

pub fn decode_validation_account(data: &[u8]) -> Option<HookDefinition> {
    let record = execute_record(data)?;
    let mut metas = Vec::new();
    let mut requires_transfer_context = false;
    for raw in record[METAS_OFFSET..].as_chunks::<META_LEN>().0 {
        if raw[33] > 1 || raw[34] > 1 {
            return None;
        }
        let config: [u8; 32] = raw[ADDRESS_CONFIG].try_into().ok()?;
        let available = 5 + metas.len();
        let address = match raw[0] {
            META_LITERAL_PUBKEY => HookAddress::Literal(config),
            META_PUBKEY_DATA => {
                requires_transfer_context = true;
                match config[0] {
                    1 => HookAddress::InstructionData { index: config[1] },
                    2 if usize::from(config[1]) < available => HookAddress::AccountData {
                        account_index: config[1],
                        data_index: config[2],
                    },
                    _ => return None,
                }
            }
            discriminator => {
                let mut program_index = None;
                if discriminator != META_PDA_THIS_PROGRAM {
                    let index = discriminator.checked_sub(META_EXTERNAL_PDA_BASE)?;
                    if usize::from(index) >= available {
                        return None;
                    }
                    requires_transfer_context |= !resolvable_execute_index(index);
                    program_index = Some(index);
                }
                let mut offset = 0;
                let mut seeds = Vec::new();
                while offset < config.len() {
                    let remaining = &config[offset..];
                    let (len, seed) = match remaining[0] {
                        0 => break,
                        1 => {
                            let len = usize::from(*remaining.get(1)?);
                            (
                                2 + len,
                                HookSeed::Literal(remaining.get(2..2 + len)?.to_vec()),
                            )
                        }
                        2 => {
                            if *remaining.get(2)? > 32 {
                                return None;
                            }
                            requires_transfer_context = true;
                            (
                                3,
                                HookSeed::InstructionData {
                                    index: remaining[1],
                                    length: remaining[2],
                                },
                            )
                        }
                        3 => {
                            let index = *remaining.get(1)?;
                            if usize::from(index) >= available {
                                return None;
                            }
                            requires_transfer_context |= !resolvable_execute_index(index);
                            (2, HookSeed::AccountKey { index })
                        }
                        4 => {
                            if usize::from(*remaining.get(1)?) >= available
                                || *remaining.get(3)? > 32
                            {
                                return None;
                            }
                            requires_transfer_context = true;
                            (
                                4,
                                HookSeed::AccountData {
                                    account_index: remaining[1],
                                    data_index: remaining[2],
                                    length: remaining[3],
                                },
                            )
                        }
                        _ => return None,
                    };
                    remaining.get(..len)?;
                    offset += len;
                    seeds.push(seed);
                    if seeds.len() > 15 {
                        return None;
                    }
                }
                HookAddress::Pda {
                    program_index,
                    seeds,
                }
            }
        };
        metas.push(ExtraMeta {
            discriminator: raw[0],
            address_config: config,
            is_signer: raw[33] == 1,
            is_writable: raw[34] == 1,
            address,
        });
    }
    Some(HookDefinition {
        metas,
        requires_transfer_context,
    })
}

const EXECUTE_MINT_INDEX: u8 = 1;
const EXECUTE_VALIDATION_INDEX: u8 = 4;

/// `spl_type_length_value` base (8-byte discriminator + 4-byte length), then the
/// `PodSlice` element count, then the elements.
const POD_SLICE_LEN_OFFSET: usize = 12;
const METAS_OFFSET: usize = 16;
const META_LEN: usize = 35;
const ADDRESS_CONFIG: std::ops::Range<usize> = 1..33;

const META_LITERAL_PUBKEY: u8 = 0;
const META_PDA_THIS_PROGRAM: u8 = 1;
const META_PUBKEY_DATA: u8 = 2;
const META_EXTERNAL_PDA_BASE: u8 = 128;

const SEED_UNINITIALIZED: u8 = 0;
const SEED_LITERAL: u8 = 1;
#[cfg(test)]
const SEED_INSTRUCTION_DATA: u8 = 2;
const SEED_ACCOUNT_KEY: u8 = 3;
#[cfg(test)]
const SEED_ACCOUNT_DATA: u8 = 4;

/// What a hook adds to the window, and whether we can build that window deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookWindow {
    /// Accounts the hook block contributes: the resolved extras, then the hook program id,
    /// then the validation-state PDA — the order `spl-transfer-hook-interface` appends them.
    pub accounts: u8,
    /// False when some meta resolves against another account's *data*. Such a set is a
    /// function of live state, so the window signed at quote time may not be the window the
    /// chain wants at land time. That is not "harder to build", it is not determinable, and
    /// it is the only hook class that fails closed.
    pub deterministic: bool,
}

pub fn extra_account_metas_pda(mint: &Pubkey, hook_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[EXTRA_ACCOUNT_METAS_SEED, mint.as_ref()], hook_program).0
}

/// Byte-in, byte-out form of [`extra_account_metas_pda`].
///
/// The orchestrator has to derive this address to know which account to fetch, but its
/// `no_sdk_leak_guard` keeps every SDK type quarantined in `dex-adapters`. Naming no
/// `solana_pubkey` in its signature is what lets it stay on the quote-core side.
pub fn extra_account_metas_pda_bytes(mint: [u8; 32], hook_program: [u8; 32]) -> [u8; 32] {
    extra_account_metas_pda(
        &Pubkey::new_from_array(mint),
        &Pubkey::new_from_array(hook_program),
    )
    .to_bytes()
}

pub fn parse_validation_account(data: &[u8]) -> Option<HookWindow> {
    let definition = decode_validation_account(data)?;
    Some(HookWindow {
        accounts: u8::try_from(definition.metas.len().checked_add(2)?).ok()?,
        deterministic: !definition.requires_transfer_context,
    })
}

fn resolvable_execute_index(index: u8) -> bool {
    index == EXECUTE_MINT_INDEX || index >= EXECUTE_VALIDATION_INDEX
}

fn resolve_seeds(
    config: &[u8],
    mint: &Pubkey,
    validation: &Pubkey,
    resolved: &[AccountMeta],
) -> Option<Vec<Vec<u8>>> {
    let mut seeds = Vec::new();
    let mut i = 0;
    while i < config.len() {
        match *config.get(i)? {
            SEED_UNINITIALIZED => break,
            SEED_LITERAL => {
                let len = usize::from(*config.get(i + 1)?);
                seeds.push(config.get(i + 2..i + 2 + len)?.to_vec());
                i += 2 + len;
            }
            SEED_ACCOUNT_KEY => {
                seeds.push(execute_account(
                    *config.get(i + 1)?,
                    mint,
                    validation,
                    resolved,
                )?);
                i += 2;
            }
            _ => return None,
        }
    }
    Some(seeds)
}

fn execute_account(
    index: u8,
    mint: &Pubkey,
    validation: &Pubkey,
    resolved: &[AccountMeta],
) -> Option<Vec<u8>> {
    match index {
        EXECUTE_MINT_INDEX => Some(mint.to_bytes().to_vec()),
        EXECUTE_VALIDATION_INDEX => Some(validation.to_bytes().to_vec()),
        _ => {
            let extra = usize::from(index.checked_sub(EXECUTE_VALIDATION_INDEX + 1)?);
            Some(resolved.get(extra)?.pubkey.to_bytes().to_vec())
        }
    }
}

pub fn resolve_hook_metas(
    data: &[u8],
    mint: &Pubkey,
    hook_program: &Pubkey,
    validation: &Pubkey,
) -> Option<Vec<AccountMeta>> {
    if decode_validation_account(data)?.requires_transfer_context {
        return None;
    }
    let data = execute_record(data)?;
    let raw = data.get(POD_SLICE_LEN_OFFSET..POD_SLICE_LEN_OFFSET + 4)?;
    let count = usize::try_from(u32::from_le_bytes(raw.try_into().ok()?)).ok()?;
    let end = METAS_OFFSET.checked_add(count.checked_mul(META_LEN)?)?;
    if data.len() < end {
        return None;
    }

    let mut resolved: Vec<AccountMeta> = Vec::with_capacity(count + 2);
    for i in 0..count {
        let meta = &data[METAS_OFFSET + i * META_LEN..METAS_OFFSET + (i + 1) * META_LEN];
        let config = &meta[ADDRESS_CONFIG];
        let (is_signer, is_writable) = (meta[33] != 0, meta[34] != 0);
        let pubkey = match meta[0] {
            META_LITERAL_PUBKEY => Pubkey::new_from_array(config.try_into().ok()?),
            META_PUBKEY_DATA => return None,
            disc => {
                let program = if disc == META_PDA_THIS_PROGRAM {
                    *hook_program
                } else {
                    let index = disc.checked_sub(META_EXTERNAL_PDA_BASE)?;
                    Pubkey::new_from_array(
                        execute_account(index, mint, validation, &resolved)?
                            .try_into()
                            .ok()?,
                    )
                };
                let seeds = resolve_seeds(config, mint, validation, &resolved)?;
                let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
                Pubkey::try_find_program_address(&refs, &program)?.0
            }
        };
        resolved.push(AccountMeta {
            pubkey,
            is_signer,
            is_writable,
        });
    }
    resolved.push(AccountMeta::new_readonly(*hook_program, false));
    resolved.push(AccountMeta::new_readonly(*validation, false));
    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(b: u8) -> Pubkey {
        Pubkey::new_from_array([b; 32])
    }

    fn account(metas: &[(u8, [u8; 32])]) -> Vec<u8> {
        let mut out = EXECUTE_DISCRIMINATOR.to_vec();
        out.extend_from_slice(&((4 + metas.len() * META_LEN) as u32).to_le_bytes());
        out.extend_from_slice(&(metas.len() as u32).to_le_bytes());
        for (disc, config) in metas {
            out.push(*disc);
            out.extend_from_slice(config);
            out.push(0);
            out.push(0);
        }
        out
    }

    fn seeds(bytes: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..bytes.len()].copy_from_slice(bytes);
        out
    }

    #[test]
    fn execute_record_may_follow_another_record_but_must_have_a_valid_header() {
        let record = account(&[(0, [7; 32])]);
        let mut data = vec![9; 8];
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(&[1, 2, 3]);
        data.extend_from_slice(&record);
        assert_eq!(
            decode_validation_account(&data),
            decode_validation_account(&record)
        );
        let mut bad = record.clone();
        bad[0] ^= 1;
        assert!(decode_validation_account(&bad).is_none());
        bad = record.clone();
        bad[8] -= 1;
        assert!(decode_validation_account(&bad).is_none());
        bad = record.clone();
        bad[49] = 2;
        assert!(decode_validation_account(&bad).is_none());
        let duplicate = [record.clone(), record].concat();
        assert!(decode_validation_account(&duplicate).is_none());
    }

    #[test]
    fn dynamic_seed_definitions_are_preserved_without_resolving_a_transfer() {
        for config in [
            seeds(&[3, 0]),
            seeds(&[3, 2]),
            seeds(&[3, 3]),
            seeds(&[2, 8, 8]),
            seeds(&[4, 1, 0, 8]),
        ] {
            let definition = decode_validation_account(&account(&[(1, config)])).unwrap();
            assert!(definition.requires_transfer_context);
            assert_eq!(definition.metas[0].address_config, config);
        }
        for config in [seeds(&[1, 0]), seeds(&[2, 1, 0])] {
            let definition = decode_validation_account(&account(&[(2, config)])).unwrap();
            assert!(definition.requires_transfer_context);
            assert_eq!(definition.metas[0].address_config, config);
        }
        assert!(decode_validation_account(&account(&[(1, seeds(&[3, 5]))])).is_none());
        assert!(decode_validation_account(&account(&[(1, seeds(&[1, 32]))])).is_none());
    }

    #[test]
    fn pda_matches_the_interface_seed() {
        let mint = pk(3);
        let program = pk(4);
        assert_eq!(
            extra_account_metas_pda(&mint, &program),
            Pubkey::find_program_address(&[b"extra-account-metas", mint.as_ref()], &program).0
        );
    }

    #[test]
    fn account_count_includes_the_hook_program_and_the_validation_pda() {
        let one = account(&[(META_LITERAL_PUBKEY, [7u8; 32])]);
        assert_eq!(
            parse_validation_account(&one).unwrap(),
            HookWindow {
                accounts: 3,
                deterministic: true
            }
        );

        assert_eq!(parse_validation_account(&account(&[])).unwrap().accounts, 2);
    }

    #[test]
    fn a_literal_and_a_mint_keyed_seed_stay_deterministic() {
        let cfg = seeds(&[
            SEED_LITERAL,
            3,
            b'a',
            b'b',
            b'c',
            SEED_ACCOUNT_KEY,
            EXECUTE_MINT_INDEX,
        ]);
        let data = account(&[(META_PDA_THIS_PROGRAM, cfg)]);
        assert!(parse_validation_account(&data).unwrap().deterministic);
    }

    #[test]
    fn a_seed_keyed_on_the_transfers_endpoints_is_not_deterministic() {
        for endpoint in [0u8, 2, 3] {
            let data = account(&[(META_PDA_THIS_PROGRAM, seeds(&[SEED_ACCOUNT_KEY, endpoint]))]);
            assert!(
                !parse_validation_account(&data).unwrap().deterministic,
                "source, destination and authority are the venue's accounts, not the \
                 mint's, so a width cached per mint cannot cover them"
            );
        }
    }

    #[test]
    fn an_instruction_data_seed_is_not_deterministic_because_this_builder_writes_no_execute_data() {
        let data = account(&[(META_PDA_THIS_PROGRAM, seeds(&[SEED_INSTRUCTION_DATA, 8, 8]))]);
        assert!(!parse_validation_account(&data).unwrap().deterministic);
    }

    #[test]
    fn a_literal_pubkey_meta_resolves_to_itself_and_the_tail_names_the_program_and_validation() {
        let (mint, hook, validation) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let literal = Pubkey::new_unique();
        let data = account(&[(META_LITERAL_PUBKEY, literal.to_bytes())]);

        let metas = resolve_hook_metas(&data, &mint, &hook, &validation).unwrap();

        assert_eq!(metas.len(), 3);
        assert_eq!(metas[0].pubkey, literal);
        assert_eq!(metas[1].pubkey, hook);
        assert_eq!(metas[2].pubkey, validation);
        assert_eq!(
            u8::try_from(metas.len()).unwrap(),
            parse_validation_account(&data).unwrap().accounts,
            "the resolved block must be exactly as wide as the width the graph budgeted"
        );
    }

    #[test]
    fn a_mint_keyed_pda_resolves_under_the_hook_program() {
        let (mint, hook, validation) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let cfg = seeds(&[
            SEED_LITERAL,
            4,
            b'p',
            b'o',
            b'o',
            b'l',
            SEED_ACCOUNT_KEY,
            EXECUTE_MINT_INDEX,
        ]);
        let data = account(&[(META_PDA_THIS_PROGRAM, cfg)]);

        let metas = resolve_hook_metas(&data, &mint, &hook, &validation).unwrap();

        assert_eq!(
            metas[0].pubkey,
            Pubkey::find_program_address(&[b"pool", mint.as_ref()], &hook).0
        );
    }

    #[test]
    fn a_later_meta_can_key_on_an_extra_the_walk_already_resolved() {
        let (mint, hook, validation) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let first = Pubkey::new_unique();
        let data = account(&[
            (META_LITERAL_PUBKEY, first.to_bytes()),
            (META_PDA_THIS_PROGRAM, seeds(&[SEED_ACCOUNT_KEY, 5])),
        ]);

        let metas = resolve_hook_metas(&data, &mint, &hook, &validation).unwrap();

        assert_eq!(
            metas[1].pubkey,
            Pubkey::find_program_address(&[first.as_ref()], &hook).0,
            "index 5 is the first extra, which the walk resolved one step earlier"
        );
    }

    #[test]
    fn the_mainnet_window_of_a_hooked_dlmm_mint_matches_what_a_landed_swap_carried() {
        const VALIDATION_ACCOUNT: &str = concat!(
            "aSVlxUv7ZhqzAAAABQAAAAEBC2hvb2tfY29uZmlnAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQELcG9v",
            "bF92YXVsdHMAAAAAAAAAAAAAAAAAAAAAAAAAAAABAQp3aGl0ZWxpc3RzAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAEBDWxwX3doaXRlbGlzdHMAAAAAAAAAAAAAAAAAAAAAAAAAAAan1RcYe9FmNdrUBFX9",
            "wsDBJMaPIVZ1pdu6y18IAAAAAAA="
        );
        let data = base64_decode(VALIDATION_ACCOUNT);
        let mint = Pubkey::from_str_const("7SckFP8Qh9Twahv516tdQXV62QnztW4Mccy5DX5YVBPg");
        let hook = Pubkey::from_str_const("8ZMvT1QczrLrnt6o32s4qAZYC8bCrGV5ZwVkSPZcQdbd");
        let validation = extra_account_metas_pda(&mint, &hook);

        assert_eq!(
            validation,
            Pubkey::from_str_const("HWYyQZjgB4AJm3MUHjsL2Z9ypwdXnZQGHNijU144W1SV")
        );
        let metas = resolve_hook_metas(&data, &mint, &hook, &validation).unwrap();

        let landed = [
            "4qwHYLfABLKCMVsxGKpgefS5zQ1azQR9YuQm4pKUKkSG",
            "Aw2WcMnRq1nk2NU1REyq92TaPhZzJTAnUEkRKW1WxdnS",
            "J5pKoWLLPXtyQe2SwCuG3RfDYMkeMFKiE2Zct9NyrJ8N",
            "BwL8iSJdKx2fNywYqba6tZo3zoQPMiY1iapWJNVc57iB",
            "Sysvar1nstructions1111111111111111111111111",
            "8ZMvT1QczrLrnt6o32s4qAZYC8bCrGV5ZwVkSPZcQdbd",
            "HWYyQZjgB4AJm3MUHjsL2Z9ypwdXnZQGHNijU144W1SV",
        ];
        assert_eq!(
            metas.iter().map(|m| m.pubkey).collect::<Vec<_>>(),
            landed.map(Pubkey::from_str_const).to_vec(),
            "these are accounts 16..23 of a swap2 that landed on pool \
             HiYHWTBnbtyQJYfw5NWLKqHij23HHc5qefab4CDVqw8w, in that order"
        );
        assert_eq!(
            parse_validation_account(&data).unwrap(),
            HookWindow {
                accounts: 7,
                deterministic: true
            }
        );
    }

    fn base64_decode(s: &str) -> Vec<u8> {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        let mut acc: u32 = 0;
        let mut bits = 0;
        for byte in s.bytes().filter(|b| *b != b'=') {
            let value = TABLE
                .iter()
                .position(|c| *c == byte)
                .expect("base64 alphabet");
            acc = (acc << 6) | value as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }

    #[test]
    fn a_pubkey_data_meta_refuses_to_resolve() {
        let (mint, hook, validation) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let data = account(&[(META_PUBKEY_DATA, [0u8; 32])]);

        assert!(resolve_hook_metas(&data, &mint, &hook, &validation).is_none());
    }

    #[test]
    fn an_account_data_seed_is_not_deterministic() {
        let cfg = seeds(&[SEED_LITERAL, 1, b'x', SEED_ACCOUNT_DATA, 2, 0, 8]);
        let data = account(&[(META_PDA_THIS_PROGRAM, cfg)]);
        let window = parse_validation_account(&data).unwrap();
        assert_eq!(window.accounts, 3, "it still occupies a window slot");
        assert!(
            !window.deterministic,
            "the resolved address depends on live account data, so the signed window \
             cannot be pinned at quote time"
        );
    }

    #[test]
    fn a_pda_under_another_listed_program_is_read_the_same_way() {
        let top_bit_index = 1u8 << 7;
        let ok = account(&[
            (META_LITERAL_PUBKEY, [7; 32]),
            (top_bit_index + 5, seeds(&[SEED_ACCOUNT_KEY, 1])),
        ]);
        assert!(parse_validation_account(&ok).unwrap().deterministic);

        let bad = account(&[
            (META_LITERAL_PUBKEY, [7; 32]),
            (top_bit_index + 5, seeds(&[SEED_ACCOUNT_DATA, 1, 0, 4])),
        ]);
        assert!(!parse_validation_account(&bad).unwrap().deterministic);

        let endpoint_program = account(&[(top_bit_index + 3, seeds(&[SEED_ACCOUNT_KEY, 1]))]);
        assert!(
            !parse_validation_account(&endpoint_program)
                .unwrap()
                .deterministic,
            "a pda under the transfer's authority is a venue account, not a mint one"
        );
    }

    #[test]
    fn pubkey_data_and_unknown_seed_kinds_fail_closed() {
        let from_data = account(&[(META_PUBKEY_DATA, seeds(&[2, 1, 0]))]);
        assert!(!parse_validation_account(&from_data).unwrap().deterministic);

        let unknown = account(&[(META_PDA_THIS_PROGRAM, seeds(&[9, 1]))]);
        assert!(parse_validation_account(&unknown).is_none());
    }

    #[test]
    fn a_truncated_or_lying_account_declines_instead_of_panicking() {
        let full = account(&[(META_LITERAL_PUBKEY, [7u8; 32]); 3]);
        for len in 0..full.len() {
            let _ = parse_validation_account(&full[..len]);
        }

        let mut lying = vec![0u8; POD_SLICE_LEN_OFFSET];
        lying.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_validation_account(&lying), None);
    }
}
