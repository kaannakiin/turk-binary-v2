use std::collections::HashSet;

use domain::Pubkey;
use domain::chain::{CLOCK_SYSVAR, SYSVAR_OWNER, is_token_program};

use crate::bytes::read_pubkey;
use crate::spec::DexSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    A,
    B,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Pool,
    Vault(Side),
    Mint(Side),
    AmmConfig,
    Observation,
    TickArray { start: i32 },
    TickArrayBitmapExtension,
    Oracle,
    BinArray { index: i64 },
    BinArrayBitmapExtension,
    DammVault(Side),
    DammVaultLp(Side),
    DammVaultLpMint(Side),
    DammVaultReserve(Side),
    DepegStake,
    PumpGlobal,
    PumpFeeConfig,
    PumpAmmGlobalConfig,
    Clock,
}

impl Role {
    /// Whether a swap on the pool can write this account. Only program and
    /// token configs, mints and sysvars are ruled out; anything else counts
    /// as written.
    #[must_use]
    pub const fn swap_writes(&self) -> bool {
        !matches!(
            self,
            Self::Mint(_)
                | Self::AmmConfig
                | Self::PumpGlobal
                | Self::PumpFeeConfig
                | Self::PumpAmmGlobalConfig
                | Self::Clock
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Pool,
    Shared,
}

/// `Optional` accounts may legitimately not exist (bitmap extensions,
/// uninitialized tick arrays); their absence still has to be confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presence {
    Required,
    Optional,
}

/// `Swap` accounts are passed to the swap instruction but never read by its math.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Need {
    Quote,
    Swap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnerRule {
    Program(Pubkey),
    TokenProgram,
    Sysvar,
}

impl OwnerRule {
    #[must_use]
    pub fn accepts(&self, owner: &Pubkey) -> bool {
        match self {
            Self::Program(program) => owner == program,
            Self::TokenProgram => is_token_program(owner),
            Self::Sysvar => owner == &SYSVAR_OWNER,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dependency {
    pub pubkey: Pubkey,
    pub role: Role,
    pub scope: Scope,
    pub presence: Presence,
    pub need: Need,
    pub owner: OwnerRule,
    pub structural: bool,
}

impl Dependency {
    #[must_use]
    pub const fn new(pubkey: Pubkey, role: Role, scope: Scope, owner: OwnerRule) -> Self {
        Self {
            pubkey,
            role,
            scope,
            presence: Presence::Required,
            need: Need::Quote,
            owner,
            structural: false,
        }
    }

    #[must_use]
    pub const fn optional(mut self) -> Self {
        self.presence = Presence::Optional;
        self
    }

    #[must_use]
    pub const fn swap_only(mut self) -> Self {
        self.need = Need::Swap;
        self
    }

    #[must_use]
    pub const fn structural(mut self) -> Self {
        self.structural = true;
        self
    }
}

/// `awaiting` lists accounts whose contents are needed to finish the
/// closure; run `closure` again once they are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closure {
    pub deps: Vec<Dependency>,
    pub awaiting: Vec<Pubkey>,
    pub verified: bool,
}

impl Closure {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.awaiting.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known<'a> {
    Unknown,
    Absent,
    Present(&'a [u8]),
}

pub trait AccountView {
    fn get(&self, key: &Pubkey) -> Known<'_>;
}

pub struct NoAccounts;

impl AccountView for NoAccounts {
    fn get(&self, _key: &Pubkey) -> Known<'_> {
        Known::Unknown
    }
}

/// Bonding curves do not store their mint, so discovery passes it in `mints`.
#[derive(Debug, Clone, Copy)]
pub struct PoolAccount<'a> {
    pub address: Pubkey,
    pub data: &'a [u8],
    pub mints: Option<(Pubkey, Pubkey)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClosureError {
    #[error("pool data ends before offset {offset}")]
    Truncated { offset: usize },
    #[error("no program address exists for {role:?}")]
    NoAddress { role: Role },
    #[error("invalid tick spacing {0}")]
    BadTickSpacing(u16),
    #[error("bitmap marks an array outside the tick range")]
    InvalidBitmap,
    #[error("unknown enum value {value} at offset {offset}")]
    UnknownVariant { offset: usize, value: u8 },
}

pub(crate) fn field(
    data: &[u8],
    offset: usize,
    role: Role,
    scope: Scope,
    owner: OwnerRule,
) -> Result<Dependency, ClosureError> {
    let pubkey = read_pubkey(data, offset).ok_or(ClosureError::Truncated { offset })?;
    Ok(Dependency::new(pubkey, role, scope, owner))
}

pub(crate) struct Builder {
    deps: Vec<Dependency>,
    awaiting: Vec<Pubkey>,
    verified: bool,
}

impl Builder {
    pub(crate) fn new(spec: &DexSpec, pool: &PoolAccount<'_>, structural: bool) -> Self {
        let mut dep = Dependency::new(
            pool.address,
            Role::Pool,
            Scope::Pool,
            OwnerRule::Program(spec.program_id),
        );
        dep.structural = structural;
        Self {
            deps: vec![dep],
            awaiting: Vec::new(),
            verified: true,
        }
    }

    pub(crate) fn push(&mut self, dep: Dependency) {
        self.deps.push(dep);
    }

    pub(crate) fn mints(&mut self, spec: &DexSpec, data: &[u8]) -> Result<(), ClosureError> {
        let (a, b) = spec.pool_mints(data).ok_or(ClosureError::Truncated {
            offset: spec.mint_offsets.map_or(0, |(_, b)| b),
        })?;
        self.mint_pair(a, b);
        Ok(())
    }

    pub(crate) fn mint_pair(&mut self, a: Pubkey, b: Pubkey) {
        for (side, mint) in [(Side::A, a), (Side::B, b)] {
            self.push(Dependency::new(
                mint,
                Role::Mint(side),
                Scope::Shared,
                OwnerRule::TokenProgram,
            ));
        }
    }

    pub(crate) fn clock(&mut self) {
        self.push(Dependency::new(
            CLOCK_SYSVAR,
            Role::Clock,
            Scope::Shared,
            OwnerRule::Sysvar,
        ));
    }

    pub(crate) fn wait_for(&mut self, key: Pubkey) {
        self.awaiting.push(key);
    }

    pub(crate) fn unverified(&mut self) {
        self.verified = false;
    }

    pub(crate) fn finish(self) -> Closure {
        let mut seen = HashSet::with_capacity(self.deps.len());
        let deps = self
            .deps
            .into_iter()
            .filter(|d| seen.insert(d.pubkey))
            .collect();
        Closure {
            deps,
            awaiting: self.awaiting,
            verified: self.verified,
        }
    }
}
