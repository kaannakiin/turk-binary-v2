use crate::{DecodeError, read_u64};

pub const ROUTE_VERSION: u8 = 1;
pub const MAX_HOPS: usize = 4;

pub(crate) const HEADER_LEN: usize = 19;
const HOP_LEN: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hop {
    pub kind: u8,
    pub hook_a: u8,
    pub hook_b: u8,
    pub tail: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    in_amount: u64,
    min_out: u64,
    hops: [Hop; MAX_HOPS],
    hop_count: u8,
}

impl Route {
    pub fn new(in_amount: u64, min_out: u64, hops: &[Hop]) -> Result<Self, DecodeError> {
        if hops.is_empty() || hops.len() > MAX_HOPS {
            return Err(DecodeError::HopCount);
        }
        let mut fixed = [Hop::default(); MAX_HOPS];
        fixed[..hops.len()].copy_from_slice(hops);
        Ok(Self {
            in_amount,
            min_out,
            hops: fixed,
            hop_count: u8::try_from(hops.len()).map_err(|_| DecodeError::HopCount)?,
        })
    }

    #[must_use]
    pub fn in_amount(&self) -> u64 {
        self.in_amount
    }

    #[must_use]
    pub fn min_out(&self) -> u64 {
        self.min_out
    }

    #[must_use]
    pub fn hops(&self) -> &[Hop] {
        &self.hops[..usize::from(self.hop_count)]
    }

    pub(crate) fn encode_into(&self, out: &mut Vec<u8>) {
        out.push(ROUTE_VERSION);
        out.extend_from_slice(&self.in_amount.to_le_bytes());
        out.extend_from_slice(&self.min_out.to_le_bytes());
        out.push(self.hop_count);
        for hop in self.hops() {
            out.extend_from_slice(&[hop.kind, hop.hook_a, hop.hook_b, hop.tail]);
        }
    }

    pub(crate) fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        let (&version, _) = data
            .get(1..)
            .and_then(<[u8]>::split_first)
            .ok_or(DecodeError::Length)?;
        if version != ROUTE_VERSION {
            return Err(DecodeError::UnsupportedVersion);
        }
        let in_amount = read_u64(data, 2)?;
        let min_out = read_u64(data, 10)?;
        let hop_count = usize::from(*data.get(18).ok_or(DecodeError::Length)?);
        if hop_count == 0 || hop_count > MAX_HOPS {
            return Err(DecodeError::HopCount);
        }
        let expected_len = hop_count
            .checked_mul(HOP_LEN)
            .and_then(|hops| hops.checked_add(HEADER_LEN))
            .ok_or(DecodeError::Length)?;
        if data.len() != expected_len {
            return Err(DecodeError::Length);
        }
        let mut hops = [Hop::default(); MAX_HOPS];
        let (encoded, _) = data[HEADER_LEN..].as_chunks::<HOP_LEN>();
        for (hop, &[kind, hook_a, hook_b, tail]) in hops.iter_mut().zip(encoded) {
            *hop = Hop {
                kind,
                hook_a,
                hook_b,
                tail,
            };
        }
        Self::new(in_amount, min_out, &hops[..hop_count])
    }
}
