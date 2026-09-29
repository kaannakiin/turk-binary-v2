use pinocchio::error::ProgramError;
use pinocchio::{AccountView, Address, ProgramResult};
use router_wire::{CONFIG_LEN, CONFIG_SEED, Config};

pub fn load(view: &AccountView, program_id: &Address) -> Result<Config, ProgramError> {
    if !view.owned_by(program_id) {
        return Err(ProgramError::InvalidAccountOwner);
    }
    let config =
        Config::decode(&view.try_borrow()?).map_err(|_| ProgramError::InvalidAccountData)?;
    let expected = Address::create_program_address(&[CONFIG_SEED, &[config.bump]], program_id)
        .map_err(|_| ProgramError::InvalidSeeds)?;
    if view.address() != &expected {
        return Err(ProgramError::InvalidSeeds);
    }
    Ok(config)
}

pub fn store(view: &mut AccountView, config: &Config) -> ProgramResult {
    let mut data = view.try_borrow_mut()?;
    if data.len() != CONFIG_LEN {
        return Err(ProgramError::InvalidAccountData);
    }
    data.copy_from_slice(&config.encode());
    Ok(())
}
