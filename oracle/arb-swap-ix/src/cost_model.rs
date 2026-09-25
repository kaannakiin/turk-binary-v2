//! Protocol constants of the scheduler's transaction cost model, shared by
//! the fire-time budget (`crates/exec`) and the offline measurement tool
//! (`onchain/tools/router-conformance`). SIMD-0186 fixes the loaded-size
//! terms; SIMD-0553 fixes the cost terms the Priority formula divides by.

/// Per-account metadata surcharge in the loaded-size accounting
/// (`TRANSACTION_ACCOUNT_BASE_SIZE`).
pub const ACCOUNT_METADATA_BYTES: u64 = 64;

/// Flat charge for each address lookup table a transaction uses: 8192 bytes
/// for a full table plus 56 of metadata.
pub const BYTES_PER_LOOKUP_TABLE: u64 = 8_248;

/// Loaded account data is charged per started 32 KiB page...
pub const LOADED_PAGE_BYTES: u64 = 32 * 1024;

/// ...at this many cost units per page.
pub const LOADED_PAGE_COST_UNITS: u64 = 8;

/// `MAX_LOADED_ACCOUNTS_DATA_SIZE_BYTES`, and the default when ComputeBudget
/// tag 4 is absent.
pub const MAX_LOADED_ACCOUNTS_DATA_SIZE_BYTES: u32 = 64 * 1024 * 1024;

pub const MAX_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

pub const SIGNATURE_COST_UNITS: u64 = 720;

/// Scheduler cost per write-locked account. Not metered at runtime; it feeds
/// the cost estimate the priority ranking divides by.
pub const WRITE_LOCK_COST_UNITS: u64 = 300;

/// Instruction data is charged one cost unit per this many bytes.
pub const INSTRUCTION_DATA_BYTES_PER_UNIT: u64 = 4;
