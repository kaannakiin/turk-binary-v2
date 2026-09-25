use std::future::Future;

use domain::{AccountUpdate, Commitment, Pubkey, Slot};
use grpc::{GroupChange, GroupKey, GrpcError, HubHandle, Placement, StreamId};
use rpc::{RpcError, RpcGateway};

/// Reads for seeds, repairs and audits. Taken at `confirmed` so they can go
/// straight into the committed layer.
pub trait AccountSource: Send + Sync + 'static {
    fn fetch(
        &self,
        keys: &[Pubkey],
        min_slot: Slot,
    ) -> impl Future<Output = Result<(Slot, Vec<Option<AccountUpdate>>), RpcError>> + Send;
}

impl AccountSource for RpcGateway {
    fn fetch(
        &self,
        keys: &[Pubkey],
        min_slot: Slot,
    ) -> impl Future<Output = Result<(Slot, Vec<Option<AccountUpdate>>), RpcError>> + Send {
        self.get_multiple_accounts_with(keys, Commitment::Confirmed, Some(min_slot))
    }
}

pub trait HubPort: Send + Sync + 'static {
    fn apply(&self, changes: Vec<GroupChange>) -> Result<(), GrpcError>;
    fn stream_for(&self, key: &GroupKey, placement: Placement) -> StreamId;
}

impl HubPort for HubHandle {
    fn apply(&self, changes: Vec<GroupChange>) -> Result<(), GrpcError> {
        Self::apply(self, changes)
    }

    fn stream_for(&self, key: &GroupKey, placement: Placement) -> StreamId {
        Self::stream_for(self, key, placement)
    }
}
