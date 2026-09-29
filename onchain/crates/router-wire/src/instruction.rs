use crate::route::HEADER_LEN;
use crate::{DecodeError, FlowRoute, Route, read_bool, read_key};

const ROUTE: u8 = 0;
const INITIALIZE: u8 = 1;
const SET_PAUSED: u8 = 2;
const SET_ADMIN: u8 = 3;
const FLOW: u8 = 4;

#[expect(
    clippy::large_enum_variant,
    reason = "FlowRoute keeps bounded steps inline so program decode does not allocate"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouterInstruction {
    Route(Route),
    Flow(FlowRoute),
    Initialize { admin: [u8; 32] },
    SetPaused { paused: bool },
    SetAdmin { new_admin: [u8; 32] },
}

impl RouterInstruction {
    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        let (&tag, rest) = data.split_first().ok_or(DecodeError::Length)?;
        match tag {
            ROUTE => Route::decode(data).map(Self::Route),
            FLOW => FlowRoute::decode(data).map(Self::Flow),
            INITIALIZE => Ok(Self::Initialize {
                admin: exact_key(rest)?,
            }),
            SET_PAUSED => match rest {
                [byte] => Ok(Self::SetPaused {
                    paused: read_bool(*byte)?,
                }),
                _ => Err(DecodeError::Length),
            },
            SET_ADMIN => Ok(Self::SetAdmin {
                new_admin: exact_key(rest)?,
            }),
            _ => Err(DecodeError::UnknownInstruction),
        }
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN);
        match self {
            Self::Route(route) => {
                out.push(ROUTE);
                route.encode_into(&mut out);
            }
            Self::Flow(route) => {
                out.push(FLOW);
                route.encode_into(&mut out);
            }
            Self::Initialize { admin } => {
                out.push(INITIALIZE);
                out.extend_from_slice(admin);
            }
            Self::SetPaused { paused } => {
                out.push(SET_PAUSED);
                out.push(u8::from(*paused));
            }
            Self::SetAdmin { new_admin } => {
                out.push(SET_ADMIN);
                out.extend_from_slice(new_admin);
            }
        }
        out
    }
}

fn exact_key(data: &[u8]) -> Result<[u8; 32], DecodeError> {
    if data.len() != 32 {
        return Err(DecodeError::Length);
    }
    read_key(data, 0)
}
