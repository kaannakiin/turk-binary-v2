pub mod adapters;
mod error;
mod instruction;
pub mod loader;
pub mod route_checks;
pub mod token_account;

pub use error::RouterError;
pub use instruction::{BuiltHop, HopInstruction, HopMeta};

pub struct HopAccountView<'a> {
    pub key: &'a [u8; 32],
    pub owner: &'a [u8; 32],
    pub is_signer: bool,
    pub is_writable: bool,
    pub data: &'a [u8],
}
