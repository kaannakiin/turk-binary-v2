use pinocchio::error::ProgramError;
use router_core::RouterError;
use router_wire::DecodeError;

pub fn custom(error: RouterError) -> ProgramError {
    ProgramError::Custom(error.into())
}

pub fn decode(error: DecodeError) -> ProgramError {
    match error {
        DecodeError::UnsupportedVersion => custom(RouterError::UnsupportedWireVersion),
        DecodeError::HopCount => custom(RouterError::BadHopCount),
        DecodeError::UnknownHopKind => custom(RouterError::UnknownHopKind),
        DecodeError::UnknownInstruction
        | DecodeError::Length
        | DecodeError::InvalidBool
        | DecodeError::Discriminator => ProgramError::InvalidInstructionData,
    }
}
