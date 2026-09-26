use std::collections::BTreeSet;

use domain::{AccountFilter, AccountUpdate, Pubkey, Slot, TxnSignature};
use yellowstone_grpc_proto::prelude::SlotStatus as ProtoSlotStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StreamId {
    Shard(u16),
    Shared(u16),
    SlotFeed,
}

/// A subscription unit: a pool with everything only it depends on, or one
/// shared account. The key decides the stream, so a group never spans two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupKey(pub Pubkey);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placement {
    Pool,
    Shared,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Group {
    pub pubkeys: BTreeSet<Pubkey>,
    pub filters: Vec<AccountFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupChange {
    Upsert {
        key: GroupKey,
        placement: Placement,
        group: Group,
    },
    Remove {
        key: GroupKey,
        placement: Placement,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    Processed,
    Confirmed,
    Finalized,
    Dead,
    Other,
}

impl From<i32> for SlotStatus {
    fn from(raw: i32) -> Self {
        match ProtoSlotStatus::try_from(raw) {
            Ok(ProtoSlotStatus::SlotProcessed) => Self::Processed,
            Ok(ProtoSlotStatus::SlotConfirmed) => Self::Confirmed,
            Ok(ProtoSlotStatus::SlotFinalized) => Self::Finalized,
            Ok(ProtoSlotStatus::SlotDead) => Self::Dead,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    /// Replay is off in config, or was turned off after the server refused it.
    ReplayDisabled,
    ReplayUnsupported,
    ReplayOutOfRange,
    /// The server accepted `from_slot` but resumed later than asked.
    ReplaySkipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitViolation {
    Pubkeys {
        limit: usize,
    },
    Filters {
        limit: usize,
    },
    RequestBytes {
        bytes: usize,
        limit: usize,
    },
    TxnFilters {
        limit: usize,
    },
    /// The server does not allow this key in a filter.
    PubkeyRejected {
        pubkey: Pubkey,
    },
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// `write_version` is node-local: it orders updates only within one
    /// `(stream, generation)`.
    Account {
        stream: StreamId,
        generation: u64,
        update: AccountUpdate,
    },
    Slot {
        stream: StreamId,
        slot: Slot,
        parent: Option<Slot>,
        status: SlotStatus,
    },
    /// Every account write of `signature` on this stream was sent before it.
    TxnCommitted {
        stream: StreamId,
        generation: u64,
        slot: Slot,
        signature: TxnSignature,
    },
    /// The server applies a filter change from `slot` on: writes after it
    /// are streamed, earlier state has to be read over RPC.
    Effective {
        stream: StreamId,
        generation: u64,
        slot: Slot,
        added: Vec<Pubkey>,
        removed: Vec<Pubkey>,
        filters_added: Vec<AccountFilter>,
    },
    Down {
        stream: StreamId,
        generation: u64,
    },
    /// A reconnect replayed what it missed, up to `at`. The plugin replays
    /// only sealed slots and drops live writes until the new filters apply,
    /// so writes to `keys` in the slot that was executing across the
    /// reconnect may still be missing.
    Resumed {
        stream: StreamId,
        generation: u64,
        from_slot: Slot,
        at: Slot,
        keys: Vec<Pubkey>,
    },
    /// A reconnect could not replay what it missed. Every key and filter of
    /// the stream may be stale; `since` is the last slot seen before the drop.
    Gap {
        stream: StreamId,
        generation: u64,
        since: Option<Slot>,
        effective: Slot,
        reason: GapReason,
        keys: Vec<Pubkey>,
        filters: Vec<AccountFilter>,
    },
    Rejected {
        stream: StreamId,
        group: GroupKey,
        reason: LimitViolation,
    },
}
