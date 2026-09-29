use crate::{DecodeError, Hop, read_u64};

pub const FLOW_ROUTE_VERSION: u8 = 1;
pub const MAX_FLOW_STEPS: usize = 16;
pub const MAX_FLOW_SLOTS: usize = 18;

pub(crate) const FLOW_HEADER_LEN: usize = 20;
const FLOW_STEP_LEN: usize = 30;

fn at(base: usize, offset: usize) -> Result<usize, DecodeError> {
    base.checked_add(offset).ok_or(DecodeError::Length)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowStep {
    pub source_slot: u8,
    pub destination_slot: u8,
    pub numerator: u64,
    pub denominator: u64,
    pub hop: Hop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowRoute {
    in_amount: u64,
    min_out: u64,
    slot_count: u8,
    steps: [FlowStep; MAX_FLOW_STEPS],
    step_count: u8,
}

impl FlowRoute {
    pub fn new(
        in_amount: u64,
        min_out: u64,
        slot_count: usize,
        steps: &[FlowStep],
    ) -> Result<Self, DecodeError> {
        if !(2..=MAX_FLOW_SLOTS).contains(&slot_count) {
            return Err(DecodeError::FlowSlotCount);
        }
        if steps.is_empty() || steps.len() > MAX_FLOW_STEPS {
            return Err(DecodeError::FlowStepCount);
        }

        let mut produced = [false; MAX_FLOW_SLOTS];
        let mut consumed = [false; MAX_FLOW_SLOTS];
        let mut last_numerator = [0u64; MAX_FLOW_SLOTS];
        let mut last_denominator = [0u64; MAX_FLOW_SLOTS];
        produced[0] = true;
        for step in steps {
            let source = usize::from(step.source_slot);
            let destination = usize::from(step.destination_slot);
            if source >= slot_count
                || destination >= slot_count
                || source == destination
                || destination == 0
                || source == 1
            {
                return Err(DecodeError::FlowGraph);
            }
            if step.denominator == 0 || step.numerator == 0 || step.numerator > step.denominator {
                return Err(DecodeError::FlowAllocation);
            }
            if !produced[source] || consumed[destination] {
                return Err(DecodeError::FlowGraph);
            }
            produced[destination] = true;
            consumed[source] = true;
            last_numerator[source] = step.numerator;
            last_denominator[source] = step.denominator;
        }
        if !produced[1]
            || (0..slot_count).any(|slot| {
                slot != 1
                    && produced[slot]
                    && (!consumed[slot] || last_numerator[slot] != last_denominator[slot])
            })
        {
            return Err(DecodeError::FlowGraph);
        }

        let mut fixed = [FlowStep::default(); MAX_FLOW_STEPS];
        fixed[..steps.len()].copy_from_slice(steps);
        Ok(Self {
            in_amount,
            min_out,
            slot_count: u8::try_from(slot_count).map_err(|_| DecodeError::FlowSlotCount)?,
            steps: fixed,
            step_count: u8::try_from(steps.len()).map_err(|_| DecodeError::FlowStepCount)?,
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
    pub fn slot_count(&self) -> usize {
        usize::from(self.slot_count)
    }

    #[must_use]
    pub fn steps(&self) -> &[FlowStep] {
        &self.steps[..usize::from(self.step_count)]
    }

    pub(crate) fn encode_into(&self, out: &mut Vec<u8>) {
        out.push(FLOW_ROUTE_VERSION);
        out.extend_from_slice(&self.in_amount.to_le_bytes());
        out.extend_from_slice(&self.min_out.to_le_bytes());
        out.push(self.slot_count);
        out.push(self.step_count);
        for step in self.steps() {
            out.extend_from_slice(&[step.source_slot, step.destination_slot]);
            out.extend_from_slice(&step.numerator.to_le_bytes());
            out.extend_from_slice(&step.denominator.to_le_bytes());
            out.extend_from_slice(&[
                step.hop.kind,
                step.hop.hook_a,
                step.hop.hook_b,
                step.hop.tail,
            ]);
            out.extend_from_slice(&step.hop.min_out.to_le_bytes());
        }
    }

    pub(crate) fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        let (&version, _) = data
            .get(1..)
            .and_then(<[u8]>::split_first)
            .ok_or(DecodeError::Length)?;
        if version != FLOW_ROUTE_VERSION {
            return Err(DecodeError::UnsupportedVersion);
        }
        let in_amount = read_u64(data, 2)?;
        let min_out = read_u64(data, 10)?;
        let slot_count = usize::from(*data.get(18).ok_or(DecodeError::Length)?);
        let step_count = usize::from(*data.get(19).ok_or(DecodeError::Length)?);
        if !(2..=MAX_FLOW_SLOTS).contains(&slot_count) {
            return Err(DecodeError::FlowSlotCount);
        }
        if step_count == 0 || step_count > MAX_FLOW_STEPS {
            return Err(DecodeError::FlowStepCount);
        }
        let expected_len = step_count
            .checked_mul(FLOW_STEP_LEN)
            .and_then(|steps| steps.checked_add(FLOW_HEADER_LEN))
            .ok_or(DecodeError::Length)?;
        if data.len() != expected_len {
            return Err(DecodeError::Length);
        }

        let mut steps = [FlowStep::default(); MAX_FLOW_STEPS];
        for (index, step) in steps.iter_mut().enumerate().take(step_count) {
            let offset = FLOW_HEADER_LEN
                .checked_add(
                    index
                        .checked_mul(FLOW_STEP_LEN)
                        .ok_or(DecodeError::Length)?,
                )
                .ok_or(DecodeError::Length)?;
            *step = FlowStep {
                source_slot: *data.get(offset).ok_or(DecodeError::Length)?,
                destination_slot: *data.get(at(offset, 1)?).ok_or(DecodeError::Length)?,
                numerator: read_u64(data, at(offset, 2)?)?,
                denominator: read_u64(data, at(offset, 10)?)?,
                hop: Hop {
                    kind: *data.get(at(offset, 18)?).ok_or(DecodeError::Length)?,
                    hook_a: *data.get(at(offset, 19)?).ok_or(DecodeError::Length)?,
                    hook_b: *data.get(at(offset, 20)?).ok_or(DecodeError::Length)?,
                    tail: *data.get(at(offset, 21)?).ok_or(DecodeError::Length)?,
                    min_out: read_u64(data, at(offset, 22)?)?,
                },
            };
        }
        Self::new(in_amount, min_out, slot_count, &steps[..step_count])
    }
}
