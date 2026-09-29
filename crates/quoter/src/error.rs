use dex::Role;
use domain::{DexKind, Pubkey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MintDecodeError {
    #[error("not owned by a token program")]
    Owner,
    #[error("unexpected mint layout")]
    Layout,
    #[error("mint is not initialized")]
    Uninitialized,
    #[error("malformed extension")]
    Extension,
    #[error("unsupported extension type {0}")]
    UnsupportedExtension(u16),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("{role:?} is owned by {owner}, not the program the venue expects")]
    Owner { role: Role, owner: Pubkey },
    #[error("{role:?} has an unexpected layout")]
    Layout { role: Role },
    #[error("{role:?} holds the wrong mint")]
    WrongMint { role: Role },
    #[error("{role:?} mint: {source}")]
    Mint { role: Role, source: MintDecodeError },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowError {
    #[error("{0} has no swap window yet")]
    Unsupported(DexKind),
    #[error("{0:?} is not known yet")]
    Incomplete(Role),
    #[error("{0:?} does not match the pool")]
    Inconsistent(Role),
    #[error("a mint of the pair has an active transfer hook")]
    TransferHook,
    #[error("the quote's tick arrays are not available in this pool state")]
    Arrays,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuoteError {
    #[error("{0} has no quote yet")]
    Unsupported(DexKind),
    #[error("{0:?} is not known yet")]
    Incomplete(Role),
    #[error("{0:?} does not match the pool")]
    Inconsistent(Role),
    #[error("the pool does not trade in this direction")]
    Disabled,
    #[error("the trade exceeds what the pool can pay")]
    Liquidity,
    #[error("the trade overflows the program's arithmetic")]
    Math,
    #[error("the swap would cross more than {0} arrays")]
    Arrays(u8),
    #[error("a mint of the pair has a transfer hook")]
    TransferHook,
    #[error("a mint of the pair is paused")]
    MintPaused,
    #[error("a mint of the pair is non-transferable")]
    NonTransferable,
    #[error("the output mint freezes new token accounts")]
    FrozenByDefault,
    #[error("a pool token account is frozen")]
    VaultFrozen,
}
