use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use domain::chain::CLOCK_SYSVAR;
use domain::{AccountFilter, Pubkey, Slot, TxnSignature};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use yellowstone_grpc_proto::prelude::{
    SubscribeRequest, SubscribeUpdate, subscribe_update::UpdateOneof,
};
use yellowstone_grpc_proto::tonic::Status;

use crate::classify::{Failure, classify};
use crate::connector::Connector;
use crate::convert::{account_update, message_lag};
use crate::events::{
    Group, GroupChange, GroupKey, LimitViolation, SlotStatus, Stamped, StreamEvent, StreamId,
};
use crate::request::{
    Built, Heartbeat, Limits, build_request, ping_request, seq_of, shape, slot_feed_request,
};
use crate::stats::Stats;
use crate::{GrpcError, GrpcSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Accounts {
        forward_clock: bool,
        txn_status: bool,
    },
    SlotFeed,
}

/// The slot feed serves every partition's global slot tree.
pub(crate) enum EventSink {
    One(mpsc::Sender<Stamped>),
    All(Vec<mpsc::Sender<Stamped>>),
}

pub(crate) struct StreamActor<C> {
    pub(crate) id: StreamId,
    pub(crate) role: Role,
    pub(crate) connector: Arc<C>,
    pub(crate) settings: Arc<GrpcSettings>,
    pub(crate) heartbeat: Heartbeat,
    pub(crate) limits: Limits,
    pub(crate) commands: mpsc::UnboundedReceiver<Vec<GroupChange>>,
    pub(crate) events: EventSink,
    pub(crate) state: State,
    pub(crate) stats: Arc<Stats>,
}

#[derive(Default)]
pub(crate) struct State {
    groups: BTreeMap<GroupKey, Group>,
    generation: u64,
    seq: u64,
    sent_keys: BTreeSet<Pubkey>,
    sent_filters: BTreeSet<AccountFilter>,
    pending: VecDeque<Pending>,
    last_slot: Option<Slot>,
    connected_before: bool,
}

struct Pending {
    seq: u64,
    sent_at: Instant,
    added: Vec<Pubkey>,
    removed: Vec<Pubkey>,
    filters_added: Vec<AccountFilter>,
}

#[derive(Default)]
struct Session {
    received: bool,
    flush_at: Option<Instant>,
    connect_seq: u64,
    gap: Option<Reconnect>,
    /// Newest slot this connection delivered. The effective slot of a
    /// reconnect must come from here, never from before the drop.
    last_slot: Option<Slot>,
}

struct Reconnect {
    since: Option<Slot>,
}

enum End {
    Shutdown,
    Dropped(Option<Status>),
}

impl<C: Connector> StreamActor<C> {
    pub(crate) async fn run(mut self) -> Result<(), GrpcError> {
        let mut failures = 0u32;
        loop {
            if self.role != Role::SlotFeed
                && self.state.groups.is_empty()
                && !self.wait_for_groups().await
            {
                return Ok(());
            }
            if !self.drain_commands().await {
                return Ok(());
            }
            let reconnect = self.state.connected_before && self.role != Role::SlotFeed;
            let since = self.state.last_slot;
            let request = match self.connect_request() {
                Ok(request) => request,
                Err(violation) => {
                    self.fit_limits(violation).await;
                    continue;
                }
            };
            let outcome = match self.connector.subscribe(request).await {
                Ok((sink, stream)) => {
                    // Attempts that failed never reached the server; the ack
                    // wait starts when a connect carries the filters.
                    let now = Instant::now();
                    for pending in &mut self.state.pending {
                        pending.sent_at = now;
                    }
                    self.state.generation += 1;
                    self.state.connected_before = true;
                    tracing::info!(stream = ?self.id, groups = self.state.groups.len(), "grpc stream connected");
                    let mut session = Session {
                        connect_seq: self.state.seq,
                        gap: reconnect.then_some(Reconnect { since }),
                        ..Session::default()
                    };
                    let end = self.pump(sink, stream, &mut session).await;
                    if !self
                        .emit(StreamEvent::Down {
                            stream: self.id,
                            generation: self.state.generation,
                        })
                        .await
                    {
                        return Ok(());
                    }
                    if session.received {
                        failures = 0;
                    }
                    match end {
                        End::Shutdown => return Ok(()),
                        End::Dropped(status) => status,
                    }
                }
                Err(status) => Some(status),
            };
            if let Some(status) = &outcome {
                tracing::warn!(stream = ?self.id, %status, "grpc stream failed");
                match classify(status) {
                    Failure::Fatal(reason) => {
                        return Err(GrpcError::Fatal {
                            stream: format!("{:?}", self.id),
                            reason,
                        });
                    }
                    Failure::Limit(violation) => self.fit_limits(violation).await,
                    Failure::Transient => {}
                }
            }
            failures += 1;
            let max = self.settings.reconnect.max_attempts;
            if max > 0 && failures >= max {
                return Err(GrpcError::GaveUp { attempts: failures });
            }
            let delay = self.settings.reconnect.delay(failures);
            tracing::warn!(stream = ?self.id, failures, ?delay, "grpc stream reconnecting");
            tokio::time::sleep(delay).await;
        }
    }

    async fn wait_for_groups(&mut self) -> bool {
        while self.state.groups.is_empty() {
            let Some(changes) = self.commands.recv().await else {
                return false;
            };
            self.apply(changes).await;
        }
        true
    }

    async fn drain_commands(&mut self) -> bool {
        loop {
            match self.commands.try_recv() {
                Ok(changes) => self.apply(changes).await,
                Err(mpsc::error::TryRecvError::Empty) => return true,
                Err(mpsc::error::TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn connect_request(&mut self) -> Result<SubscribeRequest, LimitViolation> {
        if self.role == Role::SlotFeed {
            return Ok(slot_feed_request());
        }
        let seq = self.state.seq + 1;
        let built = self.build(seq)?;
        self.record_send(seq);
        Ok(built.request)
    }

    fn build(&self, seq: u64) -> Result<Built, LimitViolation> {
        let txn_status = matches!(
            self.role,
            Role::Accounts {
                txn_status: true,
                ..
            }
        );
        let built = build_request(
            &self.state.groups,
            self.heartbeat,
            txn_status,
            self.settings.commitment,
            &self.limits,
            seq,
        )?;
        if built.bytes > self.settings.warn_request_bytes {
            tracing::warn!(stream = ?self.id, bytes = built.bytes, "grpc subscribe request is large; consider more streams");
        }
        Ok(built)
    }

    const fn txn_status(&self) -> bool {
        matches!(
            self.role,
            Role::Accounts {
                txn_status: true,
                ..
            }
        )
    }

    async fn reject_holders(&mut self, pubkey: Pubkey) {
        let holders: Vec<GroupKey> = self
            .state
            .groups
            .iter()
            .filter(|(_, g)| g.pubkeys.contains(&pubkey))
            .map(|(k, _)| *k)
            .collect();
        for group in holders {
            self.state.groups.remove(&group);
            self.emit(StreamEvent::Rejected {
                stream: self.id,
                group,
                reason: LimitViolation::PubkeyRejected { pubkey },
            })
            .await;
        }
    }

    fn record_send(&mut self, seq: u64) {
        let state = &mut self.state;
        let keys: BTreeSet<Pubkey> = state
            .groups
            .values()
            .flat_map(|g| g.pubkeys.iter().copied())
            .collect();
        let filters: BTreeSet<AccountFilter> = state
            .groups
            .values()
            .flat_map(|g| g.filters.iter().cloned())
            .collect();
        state.pending.push_back(Pending {
            seq,
            sent_at: Instant::now(),
            added: keys.difference(&state.sent_keys).copied().collect(),
            removed: state.sent_keys.difference(&keys).copied().collect(),
            filters_added: filters.difference(&state.sent_filters).cloned().collect(),
        });
        state.seq = seq;
        state.sent_keys = keys;
        state.sent_filters = filters;
    }

    /// Upserts that would push the request over a limit are refused one by
    /// one, so a single oversized pool never takes its neighbours down.
    async fn apply(&mut self, changes: Vec<GroupChange>) {
        for change in changes {
            match change {
                GroupChange::Remove { key, .. } => {
                    self.state.groups.remove(&key);
                }
                GroupChange::Upsert { key, group, .. } => {
                    let previous = self.state.groups.insert(key, group);
                    if let Err(reason) = self.build(self.state.seq + 1) {
                        match previous {
                            Some(previous) => self.state.groups.insert(key, previous),
                            None => self.state.groups.remove(&key),
                        };
                        self.emit(StreamEvent::Rejected {
                            stream: self.id,
                            group: key,
                            reason,
                        })
                        .await;
                    }
                }
            }
        }
    }

    /// The plugin words account and transaction filter limits alike, so a
    /// server-reported limit is charged to whichever part of the request
    /// actually exceeds it.
    async fn fit_limits(&mut self, violation: LimitViolation) {
        let shape = shape(&self.state.groups, self.txn_status(), &self.limits);
        match violation {
            LimitViolation::Pubkeys { limit } if shape.account_chunk > limit => {
                self.limits.pubkeys_per_filter = limit.max(1);
            }
            LimitViolation::Pubkeys { limit } => self.limits.txn_pubkeys_per_filter = limit.max(1),
            LimitViolation::Filters { limit } if shape.account_filters > limit => {
                self.limits.account_filters = Some(limit);
            }
            LimitViolation::Filters { limit } => self.limits.txn_filters = Some(limit),
            LimitViolation::PubkeyRejected { pubkey } => {
                if !(self.txn_status() && self.limits.txn_excluded.insert(pubkey)) {
                    self.reject_holders(pubkey).await;
                }
            }
            LimitViolation::RequestBytes { .. } | LimitViolation::TxnFilters { .. } => {}
        }
        while let Err(reason) = self.build(self.state.seq + 1) {
            let Some(largest) = self
                .state
                .groups
                .iter()
                .max_by_key(|(_, g)| g.pubkeys.len() + g.filters.len())
                .map(|(k, _)| *k)
            else {
                break;
            };
            self.state.groups.remove(&largest);
            self.emit(StreamEvent::Rejected {
                stream: self.id,
                group: largest,
                reason,
            })
            .await;
        }
    }

    async fn pump<Si, St>(&mut self, mut sink: Si, mut stream: St, session: &mut Session) -> End
    where
        Si: futures::Sink<SubscribeRequest, Error: std::fmt::Display> + Unpin,
        St: futures::Stream<Item = Result<SubscribeUpdate, Status>> + Unpin,
    {
        let recv_timeout = self.settings.recv_timeout();
        let mut silence = recv_timeout.map(|t| Instant::now() + t);
        loop {
            let ack_deadline = self
                .state
                .pending
                .front()
                .map(|p| p.sent_at + self.settings.filter_ack_timeout());
            tokio::select! {
                changes = self.commands.recv() => {
                    let Some(changes) = changes else { return End::Shutdown };
                    self.apply(changes).await;
                    session.flush_at.get_or_insert_with(|| Instant::now() + self.settings.filter_flush());
                }
                () = sleep_until(session.flush_at) => {
                    session.flush_at = None;
                    let seq = self.state.seq + 1;
                    let Ok(built) = self.build(seq) else { continue };
                    if let Err(err) = sink.send(built.request).await {
                        tracing::warn!(stream = ?self.id, %err, "grpc filter update failed");
                        return End::Dropped(None);
                    }
                    self.record_send(seq);
                }
                () = sleep_until(ack_deadline) => {
                    if let Some(slot) = session.last_slot {
                        tracing::warn!(stream = ?self.id, "no update tagged with the new filters; assuming they apply");
                        let seq = self.state.pending.back().map_or(self.state.seq, |p| p.seq);
                        if !self.acknowledge(seq, slot, session).await {
                            return End::Shutdown;
                        }
                    }
                }
                () = sleep_until(silence) => {
                    tracing::warn!(stream = ?self.id, ?recv_timeout, "grpc stream silent, reconnecting");
                    return End::Dropped(None);
                }
                message = stream.next() => match message {
                    Some(Ok(update)) => {
                        session.received = true;
                        silence = recv_timeout.map(|t| Instant::now() + t);
                        if let Some(end) = self.handle(update, &mut sink, session).await {
                            return end;
                        }
                    }
                    Some(Err(status)) => return End::Dropped(Some(status)),
                    None => return End::Dropped(None),
                },
            }
        }
    }

    async fn handle<Si>(
        &mut self,
        update: SubscribeUpdate,
        sink: &mut Si,
        session: &mut Session,
    ) -> Option<End>
    where
        Si: futures::Sink<SubscribeRequest, Error: std::fmt::Display> + Unpin,
    {
        if !self.check_lag(&update) {
            return Some(End::Dropped(None));
        }
        let tagged = update.filters.iter().filter_map(|name| seq_of(name)).max();
        let (slot, event) = match update.update_oneof {
            Some(UpdateOneof::Account(account)) => {
                self.stats.accounts.fetch_add(1, Ordering::Relaxed);
                let Some(mut update) = account_update(account) else {
                    tracing::warn!(stream = ?self.id, "dropping malformed grpc account update");
                    return None;
                };
                // No status ever arrives for a key left out of the status
                // filter, so its writes must not wait for one.
                if self.limits.txn_excluded.contains(&update.pubkey) {
                    update.txn = None;
                }
                let slot = update.slot;
                let forward = update.pubkey != CLOCK_SYSVAR
                    || matches!(
                        self.role,
                        Role::Accounts {
                            forward_clock: true,
                            ..
                        }
                    );
                let event = if forward {
                    StreamEvent::Account {
                        stream: self.id,
                        generation: self.state.generation,
                        update,
                    }
                } else {
                    StreamEvent::Heartbeat {
                        stream: self.id,
                        slot,
                    }
                };
                (Some(slot), Some(event))
            }
            Some(UpdateOneof::Slot(slot)) => {
                self.stats.slots.fetch_add(1, Ordering::Relaxed);
                (
                    Some(Slot(slot.slot)),
                    Some(StreamEvent::Slot {
                        stream: self.id,
                        slot: Slot(slot.slot),
                        parent: slot.parent.map(Slot),
                        status: SlotStatus::from(slot.status),
                    }),
                )
            }
            Some(UpdateOneof::TransactionStatus(status)) => {
                self.stats.statuses.fetch_add(1, Ordering::Relaxed);
                let Ok(signature) = <[u8; 64]>::try_from(status.signature.as_slice()) else {
                    tracing::warn!(stream = ?self.id, "dropping malformed grpc transaction status");
                    return None;
                };
                (
                    Some(Slot(status.slot)),
                    Some(StreamEvent::TxnCommitted {
                        stream: self.id,
                        generation: self.state.generation,
                        slot: Slot(status.slot),
                        signature: TxnSignature(signature),
                    }),
                )
            }
            Some(UpdateOneof::BlockMeta(meta)) => (
                Some(Slot(meta.slot)),
                Some(StreamEvent::Slot {
                    stream: self.id,
                    slot: Slot(meta.slot),
                    parent: Some(Slot(meta.parent_slot)),
                    status: SlotStatus::Confirmed,
                }),
            ),
            Some(UpdateOneof::Ping(_)) => {
                if let Err(err) = sink.send(ping_request(0)).await {
                    tracing::warn!(stream = ?self.id, %err, "grpc ping reply failed");
                }
                return None;
            }
            _ => return None,
        };
        if let Some(slot) = slot {
            self.observe_slot(slot, session);
            if let Some(seq) = tagged
                && !self.acknowledge(seq, slot, session).await
            {
                return Some(End::Shutdown);
            }
        }
        if let Some(event) = event
            && !self.emit(event).await
        {
            return Some(End::Shutdown);
        }
        None
    }

    /// `false` when the stream fell too far behind and must reconnect.
    fn check_lag(&self, update: &SubscribeUpdate) -> bool {
        // Slot statuses and pings carry the time the plugin sends them; only
        // account and block updates keep the time the plugin first saw them.
        let stamped = matches!(
            update.update_oneof,
            Some(UpdateOneof::Account(_) | UpdateOneof::BlockMeta(_))
        );
        let Some(lag) = update
            .created_at
            .as_ref()
            .filter(|_| stamped)
            .and_then(|created| message_lag(created, SystemTime::now()))
        else {
            return true;
        };
        if self
            .settings
            .max_message_delay()
            .is_some_and(|max| lag > max)
        {
            tracing::warn!(stream = ?self.id, ?lag, "grpc stream lagging, reconnecting");
            return false;
        }
        self.stats.lag.record(lag);
        true
    }

    fn observe_slot(&mut self, slot: Slot, session: &mut Session) {
        self.state.last_slot = self.state.last_slot.max(Some(slot));
        session.last_slot = session.last_slot.max(Some(slot));
    }

    async fn acknowledge(&mut self, seq: u64, slot: Slot, session: &mut Session) -> bool {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut filters_added = Vec::new();
        let mut any = false;
        while self.state.pending.front().is_some_and(|p| p.seq <= seq) {
            let Some(p) = self.state.pending.pop_front() else {
                break;
            };
            any = true;
            added.retain(|k| !p.removed.contains(k));
            removed.retain(|k| !p.added.contains(k));
            added.extend(p.added);
            removed.extend(p.removed);
            filters_added.extend(p.filters_added);
        }
        if !any {
            return true;
        }
        let effective = StreamEvent::Effective {
            stream: self.id,
            generation: self.state.generation,
            slot,
            added,
            removed,
            filters_added,
        };
        if !self.emit(effective).await {
            return false;
        }
        if seq >= session.connect_seq {
            session.connect_seq = 0;
            return self.flush_gap(session, slot).await;
        }
        true
    }

    async fn flush_gap(&mut self, session: &mut Session, effective: Slot) -> bool {
        let Some(Reconnect { since }) = session.gap.take() else {
            return true;
        };
        let gap = StreamEvent::Gap {
            stream: self.id,
            generation: self.state.generation,
            since,
            effective,
            keys: self.state.sent_keys.iter().copied().collect(),
            filters: self.state.sent_filters.iter().cloned().collect(),
        };
        self.emit(gap).await
    }

    async fn emit(&self, event: StreamEvent) -> bool {
        let started = Instant::now();
        let stamped = Stamped {
            sent: started.into_std(),
            event,
        };
        let sent = match &self.events {
            EventSink::One(tx) => self.send(tx, stamped).await,
            EventSink::All(txs) => {
                let mut sent = true;
                for tx in txs {
                    if !self.send(tx, stamped.clone()).await {
                        sent = false;
                        break;
                    }
                }
                sent
            }
        };
        self.stats.blocked.record(started.elapsed());
        sent
    }

    async fn send(&self, tx: &mpsc::Sender<Stamped>, event: Stamped) -> bool {
        let queued = tx.max_capacity() - tx.capacity();
        self.stats
            .queued_peak
            .fetch_max(u64::try_from(queued).unwrap_or(u64::MAX), Ordering::Relaxed);
        tx.send(event).await.is_ok()
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
