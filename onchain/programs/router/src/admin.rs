use pinocchio::cpi::{Seed, Signer};
use pinocchio::error::ProgramError;
use pinocchio::sysvars::rent::RENT_ID;
use pinocchio::{AccountView, Address, ProgramResult};
use pinocchio_system::instructions::{Allocate, Assign, CreateAccount, Transfer};
use router_core::{RouterError, loader};
use router_wire::{CONFIG_LEN, CONFIG_SEED, Config};

use crate::config;
use crate::error::custom;

const ACCOUNT_STORAGE_OVERHEAD: u64 = 128;

// pinocchio 0.11.2's `Rent` reads only the first 8 bytes of the sysvar and treats
// them as a pre-multiplied rate, so its minimum comes out at half the real one.
// The runtime computes exemption with this f64 expression; integer math would diverge.
#[expect(
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "reproduces the runtime's own f64 rent formula"
)]
fn rent_exempt_minimum(data_len: usize) -> Result<u64, ProgramError> {
    let mut rent = [0u8; 16];
    pinocchio::sysvars::get_sysvar(&mut rent, &RENT_ID, 0)?;
    let mut per_byte_year = [0u8; 8];
    per_byte_year.copy_from_slice(&rent[..8]);
    let mut threshold_years = [0u8; 8];
    threshold_years.copy_from_slice(&rent[8..]);
    let bytes = u64::try_from(data_len)
        .ok()
        .and_then(|len| len.checked_add(ACCOUNT_STORAGE_OVERHEAD))
        .ok_or(ProgramError::InvalidArgument)?;
    let per_year = bytes
        .checked_mul(u64::from_le_bytes(per_byte_year))
        .ok_or(ProgramError::InvalidArgument)?;
    Ok((per_year as f64 * f64::from_le_bytes(threshold_years)) as u64)
}

pub fn initialize(
    program_id: &Address,
    accounts: &mut [AccountView],
    admin: [u8; 32],
) -> ProgramResult {
    let [
        config_account,
        payer,
        upgrade_authority,
        system_program,
        program_data,
        ..,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !payer.is_signer() || !upgrade_authority.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if system_program.address() != &pinocchio_system::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    require_upgrade_authority(program_id, program_data, upgrade_authority)?;
    if admin == [0u8; 32] {
        return Err(custom(RouterError::ZeroAdmin));
    }
    let (expected, bump) = Address::find_program_address(&[CONFIG_SEED], program_id);
    if config_account.address() != &expected {
        return Err(ProgramError::InvalidSeeds);
    }

    let bump_seed = [bump];
    let seeds = [Seed::from(CONFIG_SEED), Seed::from(bump_seed.as_slice())];
    let signer = [Signer::from(seeds.as_slice())];
    let required = rent_exempt_minimum(CONFIG_LEN)?;
    let space = CONFIG_LEN as u64;
    let existing = config_account.lamports();
    if existing == 0 {
        CreateAccount {
            from: payer,
            to: config_account,
            lamports: required,
            space,
            owner: program_id,
        }
        .invoke_signed(&signer)?;
    } else {
        // Anyone can send lamports to the PDA first, which would make CreateAccount fail forever.
        if existing < required {
            Transfer {
                from: payer,
                to: config_account,
                lamports: required.saturating_sub(existing),
            }
            .invoke()?;
        }
        Allocate {
            account: config_account,
            space,
        }
        .invoke_signed(&signer)?;
        Assign {
            account: config_account,
            owner: program_id,
        }
        .invoke_signed(&signer)?;
    }

    config::store(
        config_account,
        &Config {
            admin,
            paused: true,
            bump,
        },
    )
}

pub fn set_paused(
    program_id: &Address,
    accounts: &mut [AccountView],
    paused: bool,
) -> ProgramResult {
    let [config_account, admin, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let mut config = require_admin(program_id, config_account, admin)?;
    config.paused = paused;
    config::store(config_account, &config)
}

pub fn set_admin(
    program_id: &Address,
    accounts: &mut [AccountView],
    new_admin: [u8; 32],
) -> ProgramResult {
    let [config_account, admin, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if new_admin == [0u8; 32] {
        return Err(custom(RouterError::ZeroAdmin));
    }
    let mut config = require_admin(program_id, config_account, admin)?;
    config.admin = new_admin;
    config::store(config_account, &config)
}

// Without this, whoever calls `initialize` first after the deploy becomes the admin.
fn require_upgrade_authority(
    program_id: &Address,
    program_data: &AccountView,
    upgrade_authority: &AccountView,
) -> ProgramResult {
    let loader = Address::new_from_array(loader::BPF_LOADER_UPGRADEABLE_ID);
    if program_data.address() != &program_data_address(program_id)
        || !program_data.owned_by(&loader)
    {
        return Err(custom(RouterError::BadProgramData));
    }
    let authority = loader::upgrade_authority(&program_data.try_borrow()?).map_err(custom)?;
    if authority.as_ref() != Some(upgrade_authority.address().as_array()) {
        return Err(custom(RouterError::NotUpgradeAuthority));
    }
    Ok(())
}

fn require_admin(
    program_id: &Address,
    config_account: &AccountView,
    admin: &AccountView,
) -> Result<Config, ProgramError> {
    if !admin.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let config = config::load(config_account, program_id)?;
    if &config.admin != admin.address().as_array() {
        return Err(custom(RouterError::NotAdmin));
    }
    Ok(config)
}

// src: solana-loader-v3-interface@8.0.1 src/lib.rs (get_program_data_address)
fn program_data_address(program_id: &Address) -> Address {
    let loader = Address::new_from_array(loader::BPF_LOADER_UPGRADEABLE_ID);
    Address::find_program_address(&[program_id.as_ref()], &loader).0
}

#[cfg(test)]
mod tests {
    use super::*;

    // src: mainnet getAccountInfo CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C (jsonParsed
    // programData = DMawCQzbgNTmbzaESc7o6pvL1KAeetY8zA7jNpzntHhU) at slot 451384914.
    #[test]
    fn derives_the_program_data_address_mainnet_reports() {
        let cpmm = Address::new_from_array([
            169, 42, 90, 139, 79, 41, 89, 82, 132, 37, 80, 170, 147, 253, 91, 149, 181, 172, 230,
            168, 235, 146, 12, 147, 148, 46, 67, 105, 12, 32, 236, 115,
        ]);
        let cpmm_program_data = Address::new_from_array([
            183, 146, 58, 164, 203, 174, 96, 70, 221, 199, 230, 45, 207, 70, 1, 246, 249, 239, 95,
            189, 16, 50, 89, 17, 169, 68, 97, 228, 232, 153, 18, 19,
        ]);
        assert_eq!(program_data_address(&cpmm), cpmm_program_data);
    }
}
