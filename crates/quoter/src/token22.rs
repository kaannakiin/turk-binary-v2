use domain::Pubkey;
use domain::chain::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};

use crate::error::{MintDecodeError, QuoteError};

// src: spl-token-interface@3.0.0 src/state.rs (Mint::LEN, Mint::unpack_from_slice)
const MINT_LEN: usize = 82;
const MINT_AUTHORITY_TAG: usize = 0;
const DECIMALS: usize = 44;
const IS_INITIALIZED: usize = 45;
const FREEZE_AUTHORITY_TAG: usize = 46;
// src: spl-token-2022-interface@3.1.2 src/extension/mod.rs (BASE_ACCOUNT_LENGTH, AccountType::Mint)
const ACCOUNT_TYPE: usize = 165;
const ACCOUNT_TYPE_MINT: u8 = 1;
// src: spl-token-2022-interface@3.1.2 src/extension/mod.rs (ExtensionType)
const EXT_TRANSFER_FEE_CONFIG: u16 = 1;
const EXT_DEFAULT_ACCOUNT_STATE: u16 = 6;
const EXT_NON_TRANSFERABLE: u16 = 9;
const EXT_TRANSFER_HOOK: u16 = 14;
const EXT_TOKEN_METADATA: u16 = 19;
const EXT_PAUSABLE: u16 = 26;
// src: spl-token-2022-interface@3.1.2 src/extension/transfer_fee/mod.rs (TransferFeeConfig, TransferFee)
const OLDER_FEE: usize = 72;
const NEWER_FEE: usize = 90;
// src: spl-token-2022-interface@3.1.2 src/extension/transfer_hook/mod.rs (TransferHook)
const HOOK_PROGRAM: usize = 32;
// src: spl-token-2022-interface@3.1.2 src/extension/pausable/mod.rs (PausableConfig: authority, paused)
const PAUSED: usize = 32;
// src: spl-token-2022-interface@3.1.2 src/extension/default_account_state/mod.rs (DefaultAccountState),
// src/state.rs (AccountState::Frozen = 2)
const ACCOUNT_STATE_FROZEN: u8 = 2;
// src: spl-token-2022-interface@3.1.2 src/extension/transfer_fee/mod.rs (ONE_IN_BASIS_POINTS)
const ONE_IN_BASIS_POINTS: u128 = 10_000;

/// TLV payload length of every sized mint extension. Account-only types are
/// an error on a mint; types added after 3.1.2 are unsupported until checked.
// src: spl-token-2022-interface@3.1.2 src/extension/mod.rs (ExtensionType::try_get_type_len, get_account_type)
fn mint_extension_len(kind: u16) -> Result<Option<usize>, MintDecodeError> {
    Ok(Some(match kind {
        1 => 108,
        3 | 12 | 28 => 32,
        4 => 65,
        6 => 1,
        9 => 0,
        10 => 52,
        16 => 129,
        14 | 18 | 20 | 22 => 64,
        19 => return Ok(None),
        21 => 80,
        23 => 72,
        24 => 196,
        25 => 56,
        26 => 33,
        2 | 5 | 7 | 8 | 11 | 13 | 15 | 17 | 27 => return Err(MintDecodeError::Extension),
        _ => return Err(MintDecodeError::UnsupportedExtension(kind)),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferFee {
    pub epoch: u64,
    pub maximum_fee: u64,
    pub basis_points: u16,
}

// src: spl-token-2022-interface@3.1.2 src/extension/transfer_fee/mod.rs (TransferFee::ceil_div, calculate_fee)
impl TransferFee {
    fn ceil_div(numerator: u128, denominator: u128) -> Option<u128> {
        numerator
            .checked_add(denominator)?
            .checked_sub(1)?
            .checked_div(denominator)
    }

    pub(crate) fn calculate_fee(&self, pre_fee_amount: u64) -> Option<u64> {
        let basis_points = u128::from(self.basis_points);
        if basis_points == 0 || pre_fee_amount == 0 {
            Some(0)
        } else {
            let numerator = u128::from(pre_fee_amount).checked_mul(basis_points)?;
            let raw_fee: u64 = Self::ceil_div(numerator, ONE_IN_BASIS_POINTS)?
                .try_into()
                .ok()?;
            Some(raw_fee.min(self.maximum_fee))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferFeeSchedule {
    pub older: TransferFee,
    pub newer: TransferFee,
}

impl TransferFeeSchedule {
    // src: spl-token-2022-interface@3.1.2 src/extension/transfer_fee/mod.rs (TransferFeeConfig::get_epoch_fee)
    pub(crate) fn at_epoch(&self, epoch: u64) -> TransferFee {
        if epoch >= self.newer.epoch {
            self.newer
        } else {
            self.older
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferHook {
    pub authority: Option<Pubkey>,
    pub program: Option<Pubkey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mint {
    pub token_2022: bool,
    pub decimals: u8,
    pub supply: u64,
    pub freeze_authority: Option<Pubkey>,
    pub transfer_fee: Option<TransferFeeSchedule>,
    pub transfer_hook: Option<TransferHook>,
    pub restrictions: Restrictions,
    pub extensions: Vec<u16>,
}

/// What stops Token-2022 moving the mint: `Pausable` while paused,
/// `NonTransferable`, and a `DefaultAccountState` of frozen for new accounts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Restrictions {
    pub paused: bool,
    pub non_transferable: bool,
    pub default_frozen: bool,
}

impl Mint {
    pub(crate) fn fee_at(&self, epoch: u64) -> Option<TransferFee> {
        self.transfer_fee.map(|s| s.at_epoch(epoch))
    }

    /// A hook whose program is unset runs nothing.
    pub(crate) fn has_active_hook(&self) -> bool {
        self.transfer_hook.is_some_and(|h| h.program.is_some())
    }
}

/// Whether Token-2022 would move both mints of a swap paying out `output`.
/// A mint that freezes new accounts refuses only as the output: the swap may
/// have to create the account it pays into, and an input account already
/// holds a balance.
// src: solana-program/token-2022@f4a1c94a10c43eb325f72d5a731055a869cb8b0f program/src/processor.rs
// (process_transfer: MintPaused, NonTransferable; _process_initialize_account: DefaultAccountState)
pub(crate) fn check_transfer(input: &Mint, output: &Mint) -> Result<(), QuoteError> {
    for mint in [input, output] {
        if mint.has_active_hook() {
            return Err(QuoteError::TransferHook);
        }
        if mint.restrictions.paused {
            return Err(QuoteError::MintPaused);
        }
        if mint.restrictions.non_transferable {
            return Err(QuoteError::NonTransferable);
        }
    }
    if output.restrictions.default_frozen {
        return Err(QuoteError::FrozenByDefault);
    }
    Ok(())
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn u64_at(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn pubkey_at(data: &[u8], offset: usize) -> Option<Pubkey> {
    let bytes: [u8; 32] = data.get(offset..offset + 32)?.try_into().ok()?;
    (bytes != [0; 32]).then(|| Pubkey::new_from_array(bytes))
}

fn read_fee(data: &[u8], offset: usize) -> Option<TransferFee> {
    Some(TransferFee {
        epoch: u64_at(data, offset)?,
        maximum_fee: u64_at(data, offset + 8)?,
        basis_points: u16_at(data, offset + 16)?,
    })
}

// src: spl-token-metadata-interface@1.0.0 src/state.rs (TokenMetadata: update_authority, mint, name, symbol, uri, additional_metadata)
fn valid_metadata(bytes: &[u8]) -> bool {
    fn string(bytes: &mut &[u8]) -> Option<()> {
        let len = usize::try_from(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?)).ok()?;
        std::str::from_utf8(bytes.get(4..4usize.checked_add(len)?)?).ok()?;
        *bytes = bytes.get(4 + len..)?;
        Some(())
    }
    let Some(mut rest) = bytes.get(64..) else {
        return false;
    };
    for _ in 0..3 {
        if string(&mut rest).is_none() {
            return false;
        }
    }
    let Some(count) = rest
        .get(..4)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_le_bytes)
        .and_then(|c| usize::try_from(c).ok())
    else {
        return false;
    };
    rest = &rest[4..];
    if count > rest.len() / 8 {
        return false;
    }
    for _ in 0..count {
        if string(&mut rest).is_none() || string(&mut rest).is_none() {
            return false;
        }
    }
    rest.is_empty()
}

pub(crate) fn decode_mint(owner: &Pubkey, data: &[u8]) -> Result<Mint, MintDecodeError> {
    let token_2022 = if *owner == TOKEN_PROGRAM {
        false
    } else if *owner == TOKEN_2022_PROGRAM {
        true
    } else {
        return Err(MintDecodeError::Owner);
    };
    if data.len() < MINT_LEN || (!token_2022 && data.len() != MINT_LEN) {
        return Err(MintDecodeError::Layout);
    }
    for tag in [MINT_AUTHORITY_TAG, FREEZE_AUTHORITY_TAG] {
        if !matches!(&data[tag..tag + 4], [0 | 1, 0, 0, 0]) {
            return Err(MintDecodeError::Layout);
        }
    }
    if data[IS_INITIALIZED] != 1 {
        return Err(MintDecodeError::Uninitialized);
    }
    let mut mint = Mint {
        token_2022,
        decimals: data[DECIMALS],
        supply: u64_at(data, 36).ok_or(MintDecodeError::Layout)?,
        freeze_authority: if data[FREEZE_AUTHORITY_TAG] == 1 {
            pubkey_at(data, FREEZE_AUTHORITY_TAG + 4)
        } else {
            None
        },
        transfer_fee: None,
        transfer_hook: None,
        restrictions: Restrictions::default(),
        extensions: Vec::new(),
    };
    if data.len() == MINT_LEN {
        return Ok(mint);
    }
    if data.len() <= ACCOUNT_TYPE
        || data[ACCOUNT_TYPE] != ACCOUNT_TYPE_MINT
        || data[MINT_LEN..ACCOUNT_TYPE].iter().any(|b| *b != 0)
    {
        return Err(MintDecodeError::Layout);
    }
    let mut tlv = &data[ACCOUNT_TYPE + 1..];
    while !tlv.is_empty() && tlv.iter().any(|b| *b != 0) {
        let kind = u16_at(tlv, 0).ok_or(MintDecodeError::Extension)?;
        let len = usize::from(u16_at(tlv, 2).ok_or(MintDecodeError::Extension)?);
        if kind == 0 || mint.extensions.contains(&kind) {
            return Err(MintDecodeError::Extension);
        }
        let payload = tlv.get(4..4 + len).ok_or(MintDecodeError::Extension)?;
        if mint_extension_len(kind)?.is_some_and(|expected| expected != len)
            || (kind == EXT_TOKEN_METADATA && !valid_metadata(payload))
        {
            return Err(MintDecodeError::Extension);
        }
        mint.extensions.push(kind);
        match kind {
            EXT_TRANSFER_FEE_CONFIG => {
                let older = read_fee(payload, OLDER_FEE).ok_or(MintDecodeError::Extension)?;
                let newer = read_fee(payload, NEWER_FEE).ok_or(MintDecodeError::Extension)?;
                if u128::from(older.basis_points) > ONE_IN_BASIS_POINTS
                    || u128::from(newer.basis_points) > ONE_IN_BASIS_POINTS
                {
                    return Err(MintDecodeError::Extension);
                }
                mint.transfer_fee = Some(TransferFeeSchedule { older, newer });
            }
            EXT_TRANSFER_HOOK => {
                mint.transfer_hook = Some(TransferHook {
                    authority: pubkey_at(payload, 0),
                    program: pubkey_at(payload, HOOK_PROGRAM),
                });
            }
            EXT_PAUSABLE => mint.restrictions.paused = payload[PAUSED] != 0,
            EXT_NON_TRANSFERABLE => mint.restrictions.non_transferable = true,
            EXT_DEFAULT_ACCOUNT_STATE => {
                mint.restrictions.default_frozen = payload[0] == ACCOUNT_STATE_FROZEN;
            }
            _ => {}
        }
        tlv = &tlv[4 + len..];
    }
    Ok(mint)
}
