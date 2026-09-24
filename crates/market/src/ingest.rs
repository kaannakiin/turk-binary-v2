use std::collections::HashMap;

use domain::{Commitment, Pubkey};
use grpc::{HubHandle, SlotStatus, StreamEvent, SubscriptionTarget};
use rpc::RpcGateway;
use tokio::sync::mpsc;

use crate::fork::SlotTree;
use crate::store::Source;
use crate::{AccountStore, MarketError, Stats, Universe};

const POOLS_SUBSCRIPTION: &str = "pools";

/// Subscribe first, snapshot second: updates that land while the snapshot is
/// in flight queue up in the stream, and the store's slot ordering discards
/// whichever copy is older, so there is no gap between the two.
pub async fn run(
    universe: &Universe,
    rpc: &RpcGateway,
    hub: &HubHandle,
    events: &mut mpsc::Receiver<StreamEvent>,
    store: &AccountStore,
    stats: &Stats,
) -> Result<(), MarketError> {
    let keys = universe.pool_keys();
    hub.subscribe(
        POOLS_SUBSCRIPTION,
        SubscriptionTarget::Pubkeys(keys.clone()),
    )
    .await?;
    snapshot(rpc, &keys, store).await?;
    // One tree per shard: write_version (and Alpenglow's bank_id) only mean
    // something within a single connection.
    let mut trees: HashMap<usize, SlotTree> = HashMap::new();
    while let Some(event) = events.recv().await {
        match event {
            StreamEvent::Account {
                shard,
                generation,
                update,
            } => {
                let dex = dex::identify(&update.owner, &update.data);
                let accepted = store.apply_stream(Source { shard, generation }, update);
                stats.record_update(dex, accepted);
            }
            StreamEvent::Slot {
                shard,
                slot,
                parent,
                status,
            } => {
                let tree = trees.entry(shard).or_default();
                if let Some(parent) = parent {
                    tree.record_parent(slot, parent);
                }
                match status {
                    SlotStatus::Processed => stats.record_slot(slot),
                    SlotStatus::Confirmed | SlotStatus::Finalized => {
                        if let Some(resolution) = tree.confirm(slot) {
                            let resolved = store.resolve(shard, &resolution);
                            stats.record_confirmation(slot, resolved, resolution.has_gap());
                        }
                    }
                    SlotStatus::Dead => stats.record_dead(store.drop_slot(shard, slot)),
                    SlotStatus::Other => {}
                }
            }
            StreamEvent::Reconnected { shard, accounts } => {
                stats.record_reconnect();
                tracing::info!(
                    shard,
                    accounts = accounts.len(),
                    "re-snapshotting reconnected shard"
                );
                snapshot(rpc, &accounts, store).await?;
            }
        }
    }
    Err(MarketError::StreamClosed)
}

/// Taken at `confirmed` so the snapshot can go straight into the committed
/// layer without its own fork check.
async fn snapshot(
    rpc: &RpcGateway,
    keys: &[Pubkey],
    store: &AccountStore,
) -> Result<(), MarketError> {
    let accounts = rpc
        .get_multiple_accounts_at(keys, Commitment::Confirmed)
        .await?;
    let mut missing = 0usize;
    for account in accounts {
        match account {
            Some(update) => {
                store.apply_confirmed(update);
            }
            None => missing += 1,
        }
    }
    if missing > 0 {
        tracing::warn!(missing, "pools missing from rpc snapshot");
    }
    tracing::info!(accounts = store.len(), "rpc snapshot applied");
    Ok(())
}
