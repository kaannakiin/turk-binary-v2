use domain::Pubkey;

use crate::closure::{ClosureError, Role};

pub(crate) fn pda(seeds: &[&[u8]], program: &Pubkey, role: Role) -> Result<Pubkey, ClosureError> {
    Pubkey::try_find_program_address(seeds, program)
        .map(|(address, _bump)| address)
        .ok_or(ClosureError::NoAddress { role })
}
