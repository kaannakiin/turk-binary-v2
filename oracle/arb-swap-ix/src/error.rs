use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PoolsError {
    #[error("buffer too small: expected at least {expected} bytes, got {got}")]
    BufferTooSmall { expected: usize, got: usize },

    #[error("unrecognized meteora bin array: size {size}, disc {disc:02x?}")]
    UnrecognizedBinArray { size: usize, disc: [u8; 8] },

    #[error("unrecognized whirlpool tick array: size {size}, disc {disc:02x?}")]
    UnrecognizedTickArray { size: usize, disc: [u8; 8] },

    #[error("unrecognized meteora damm-v1 vault: size {size}, disc {disc:02x?}")]
    UnrecognizedDammV1Vault { size: usize, disc: [u8; 8] },

    #[error("unrecognized meteora damm-v1 pool: size {size}, disc {disc:02x?}")]
    UnrecognizedMeteoraDammV1Pool { size: usize, disc: [u8; 8] },

    #[error("unrecognized meteora damm-v2 pool: size {size}, disc {disc:02x?}")]
    UnrecognizedMeteoraDammV2Pool { size: usize, disc: [u8; 8] },

    #[error("unrecognized meteora dlmm lb_pair: size {size}, disc {disc:02x?}")]
    UnrecognizedLbPair { size: usize, disc: [u8; 8] },

    #[error("unrecognized whirlpool: size {size}, disc {disc:02x?}")]
    UnrecognizedWhirlpool { size: usize, disc: [u8; 8] },

    #[error("unrecognized raydium clmm pool: size {size}, disc {disc:02x?}")]
    UnrecognizedRaydiumClmmPool { size: usize, disc: [u8; 8] },

    #[error("unrecognized raydium cpmm pool: size {size}, disc {disc:02x?}")]
    UnrecognizedRaydiumCpmmPool { size: usize, disc: [u8; 8] },

    #[error("unrecognized pump swap pool: size {size}, disc {disc:02x?}")]
    UnrecognizedPumpSwapPool { size: usize, disc: [u8; 8] },
}
