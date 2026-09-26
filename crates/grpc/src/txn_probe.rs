use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;

use domain::chain::CLOCK_SYSVAR;
use domain::{Commitment, Pubkey};
use futures::StreamExt;
use tokio::time::Instant;
use yellowstone_grpc_proto::prelude::{
    SubscribeRequest, SubscribeUpdate, SubscribeUpdateAccount, SubscribeUpdateTransactionInfo,
    subscribe_update::UpdateOneof,
};
use yellowstone_grpc_proto::prost_types::Timestamp;

use crate::connector::{Connector, TonicConnector};
use crate::events::{Group, GroupKey, LimitViolation, SlotStatus};
use crate::probe::Finding;
use crate::request::{Heartbeat, Limits, build_request};
use crate::{GrpcError, GrpcSettings};

const CHECK: &str = "txn groups";
const CLOSED_SLOTS_KEPT: u64 = 64;

#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub label: String,
    pub pool_keys: Vec<Pubkey>,
    pub shared_keys: Vec<Pubkey>,
}

#[derive(Debug, Clone, Copy)]
pub struct TxnProbeOptions {
    pub duration: Duration,
    pub orphan_after: Duration,
    pub slot_statuses: bool,
    /// Also stream full transactions, for their post token balances.
    pub record: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conn {
    Pool,
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceKind {
    Account,
    Txn,
    TxnStatus,
    Slot,
}

#[derive(Debug, Clone)]
pub struct TraceRow {
    pub conn: Conn,
    pub recv_unix_us: u128,
    pub created_unix_us: Option<u128>,
    pub kind: TraceKind,
    pub slot: u64,
    pub write_version: Option<u64>,
    pub signature: Option<Vec<u8>>,
    pub pubkey: Option<Pubkey>,
    pub owner: Option<Pubkey>,
    pub lamports: Option<u64>,
    pub data: Option<Bytes>,
    pub token_balances: Vec<(Pubkey, u64)>,
    pub slot_status: Option<SlotStatus>,
}

/// Measures how a transaction's account writes and its status message
/// arrive on one stream. Read-only: it only opens subscriptions.
pub async fn probe_txn_groups(
    endpoint: String,
    x_token: Option<String>,
    settings: &GrpcSettings,
    targets: &[ProbeTarget],
    options: TxnProbeOptions,
    trace: &mut dyn FnMut(TraceRow),
) -> Result<Vec<Finding>, GrpcError> {
    let connector = TonicConnector::new(endpoint, x_token, settings)?;
    let limits = Limits::from_settings(settings);
    let mut findings = vec![match connector.version().await {
        Ok(version) => Finding::new("provider version", true, version),
        Err(status) => Finding::new("provider version", false, format!("refused: {status}")),
    }];
    let requests = pool_request(targets, &limits, options.slot_statuses, options.record)
        .and_then(|pool| Ok((pool, shared_request(targets, &limits)?)));
    let (pool_request, shared_request) = match requests {
        Ok(requests) => requests,
        Err(violation) => {
            findings.push(Finding::new(CHECK, false, format!("{violation:?}")));
            return Ok(findings);
        }
    };
    let (_pool_sink, pool_stream) = match connector.subscribe(pool_request).await {
        Ok(opened) => opened,
        Err(status) => {
            findings.push(Finding::new(
                CHECK,
                false,
                format!("pool stream refused: {status}"),
            ));
            return Ok(findings);
        }
    };
    let (_shared_sink, shared_stream) = match connector.subscribe(shared_request).await {
        Ok(opened) => opened,
        Err(status) => {
            findings.push(Finding::new(
                CHECK,
                false,
                format!("shared stream refused: {status}"),
            ));
            return Ok(findings);
        }
    };
    let mut merged = futures::stream::select(
        pool_stream.map(|m| (Conn::Pool, m)),
        shared_stream.map(|m| (Conn::Shared, m)),
    );
    let mut tally = Tally::new(targets, options.orphan_after);
    let deadline = tokio::time::sleep(options.duration);
    tokio::pin!(deadline);
    let mut sweep = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            () = &mut deadline => break,
            _ = sweep.tick() => tally.sweep(Instant::now()),
            next = merged.next() => match next {
                Some((conn, Ok(update))) => {
                    let now = Instant::now();
                    if let Some(row) = trace_row(conn, &update) {
                        trace(row);
                    }
                    tally.observe(conn, &update, now);
                }
                Some((conn, Err(status))) => {
                    findings.push(Finding::new(CHECK, false, format!("{conn:?} stream ended early: {status}")));
                    break;
                }
                None => break,
            },
        }
    }
    findings.extend(tally.findings(options.slot_statuses));
    Ok(findings)
}

fn pool_request(
    targets: &[ProbeTarget],
    limits: &Limits,
    slot_statuses: bool,
    record: bool,
) -> Result<SubscribeRequest, LimitViolation> {
    let heartbeat = if slot_statuses {
        Heartbeat::Slots
    } else {
        Heartbeat::Clock
    };
    let keys: BTreeSet<Pubkey> = targets
        .iter()
        .flat_map(|t| t.pool_keys.iter().copied())
        .collect();
    let mut request = build_request(
        &groups(&keys),
        heartbeat,
        true,
        Commitment::Processed,
        limits,
        1,
    )?
    .request;
    if record {
        request.transactions = request.transactions_status.clone();
    }
    Ok(request)
}

fn shared_request(
    targets: &[ProbeTarget],
    limits: &Limits,
) -> Result<SubscribeRequest, LimitViolation> {
    let keys: BTreeSet<Pubkey> = targets
        .iter()
        .flat_map(|t| t.shared_keys.iter().copied())
        .collect();
    Ok(build_request(
        &groups(&keys),
        Heartbeat::Clock,
        false,
        Commitment::Processed,
        limits,
        1,
    )?
    .request)
}

fn groups(keys: &BTreeSet<Pubkey>) -> BTreeMap<GroupKey, Group> {
    keys.first()
        .map(|&first| {
            BTreeMap::from([(
                GroupKey(first),
                Group {
                    pubkeys: keys.clone(),
                    filters: Vec::new(),
                },
            )])
        })
        .unwrap_or_default()
}

/// Balance indexes run over the static keys, then the loaded writable and
/// loaded readonly addresses.
fn post_token_balances(info: &SubscribeUpdateTransactionInfo) -> Vec<(Pubkey, u64)> {
    let (Some(message), Some(meta)) = (
        info.transaction.as_ref().and_then(|t| t.message.as_ref()),
        info.meta.as_ref(),
    ) else {
        return Vec::new();
    };
    let keys: Vec<&Vec<u8>> = message
        .account_keys
        .iter()
        .chain(&meta.loaded_writable_addresses)
        .chain(&meta.loaded_readonly_addresses)
        .collect();
    meta.post_token_balances
        .iter()
        .filter_map(|balance| {
            let key = keys.get(usize::try_from(balance.account_index).ok()?)?;
            let amount = balance.ui_token_amount.as_ref()?.amount.parse().ok()?;
            Some((Pubkey::try_from(key.as_slice()).ok()?, amount))
        })
        .collect()
}

fn unix_us(timestamp: &Timestamp) -> Option<u128> {
    let secs = u128::try_from(timestamp.seconds).ok()?;
    let nanos = u128::try_from(timestamp.nanos).ok()?;
    Some(secs * 1_000_000 + nanos / 1_000)
}

fn trace_row(conn: Conn, update: &SubscribeUpdate) -> Option<TraceRow> {
    let recv_unix_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_micros();
    let created_unix_us = update.created_at.as_ref().and_then(unix_us);
    let row = |kind, slot| TraceRow {
        conn,
        recv_unix_us,
        created_unix_us,
        kind,
        slot,
        write_version: None,
        signature: None,
        pubkey: None,
        owner: None,
        lamports: None,
        data: None,
        token_balances: Vec::new(),
        slot_status: None,
    };
    match update.update_oneof.as_ref()? {
        UpdateOneof::Account(account) => {
            let info = account.account.as_ref()?;
            Some(TraceRow {
                write_version: Some(info.write_version),
                signature: info.txn_signature.clone(),
                pubkey: Pubkey::try_from(info.pubkey.as_slice()).ok(),
                owner: Pubkey::try_from(info.owner.as_slice()).ok(),
                lamports: Some(info.lamports),
                data: Some(info.data.clone()),
                ..row(TraceKind::Account, account.slot)
            })
        }
        UpdateOneof::Transaction(txn) => {
            let info = txn.transaction.as_ref()?;
            Some(TraceRow {
                signature: Some(info.signature.clone()),
                token_balances: post_token_balances(info),
                ..row(TraceKind::Txn, txn.slot)
            })
        }
        UpdateOneof::TransactionStatus(status) => Some(TraceRow {
            signature: Some(status.signature.clone()),
            ..row(TraceKind::TxnStatus, status.slot)
        }),
        UpdateOneof::Slot(slot) => Some(TraceRow {
            slot_status: Some(SlotStatus::from(slot.status)),
            ..row(TraceKind::Slot, slot.slot)
        }),
        _ => None,
    }
}

type GroupId = (u64, Vec<u8>);

struct Open {
    last_seq: u64,
    contiguous: bool,
    opened: Instant,
    last_at: Instant,
    per_target: BTreeMap<usize, u64>,
}

#[derive(Debug, Default)]
struct TargetStats {
    groups: u64,
    multi: u64,
    noncontiguous: u64,
    pool_updates: u64,
    mixed: u64,
    orphans: u64,
    unsigned: u64,
    delays_us: Vec<u64>,
    superseded: u64,
    wrong_release: u64,
    early_by_us: Vec<u64>,
}

impl TargetStats {
    fn merge(&mut self, other: &mut Self) {
        self.groups += other.groups;
        self.multi += other.multi;
        self.noncontiguous += other.noncontiguous;
        self.pool_updates += other.pool_updates;
        self.mixed += other.mixed;
        self.orphans += other.orphans;
        self.unsigned += other.unsigned;
        self.delays_us.append(&mut other.delays_us);
        self.superseded += other.superseded;
        self.wrong_release += other.wrong_release;
        self.early_by_us.append(&mut other.early_by_us);
    }
}

struct Tally {
    owner: HashMap<Pubkey, usize>,
    shared: HashSet<Pubkey>,
    labels: Vec<String>,
    orphan_after: Duration,
    seq: u64,
    max_slot: u64,
    open: HashMap<GroupId, Open>,
    closed: HashMap<GroupId, Instant>,
    processed: BTreeSet<u64>,
    stats: Vec<TargetStats>,
    late_accounts: u64,
    statuses: u64,
    statuses_after_processed: u64,
    statuses_without_writes: u64,
    shared_before_status: u64,
    shared_after_status: u64,
    shared_skew_us: Vec<u64>,
    latest: HashMap<usize, GroupId>,
    superseded: HashMap<GroupId, Instant>,
}

impl Tally {
    fn new(targets: &[ProbeTarget], orphan_after: Duration) -> Self {
        let mut owner = HashMap::new();
        for (i, target) in targets.iter().enumerate() {
            for key in &target.pool_keys {
                owner.entry(*key).or_insert(i);
            }
        }
        Self {
            owner,
            shared: targets
                .iter()
                .flat_map(|t| t.shared_keys.iter().copied())
                .collect(),
            labels: targets.iter().map(|t| t.label.clone()).collect(),
            orphan_after,
            seq: 0,
            max_slot: 0,
            open: HashMap::new(),
            closed: HashMap::new(),
            processed: BTreeSet::new(),
            stats: targets.iter().map(|_| TargetStats::default()).collect(),
            late_accounts: 0,
            statuses: 0,
            statuses_after_processed: 0,
            statuses_without_writes: 0,
            shared_before_status: 0,
            shared_after_status: 0,
            shared_skew_us: Vec::new(),
            latest: HashMap::new(),
            superseded: HashMap::new(),
        }
    }

    fn observe(&mut self, conn: Conn, update: &SubscribeUpdate, now: Instant) {
        match (conn, &update.update_oneof) {
            (_, Some(UpdateOneof::Account(account))) => self.account(conn, account, now),
            (Conn::Pool, Some(UpdateOneof::TransactionStatus(status))) => {
                self.status((status.slot, status.signature.clone()), now);
            }
            (Conn::Pool, Some(UpdateOneof::Slot(slot))) => {
                if SlotStatus::from(slot.status) == SlotStatus::Processed {
                    self.processed.insert(slot.slot);
                }
                self.advance(slot.slot);
            }
            _ => {}
        }
    }

    fn account(&mut self, conn: Conn, account: &SubscribeUpdateAccount, now: Instant) {
        let Some(info) = &account.account else { return };
        let Ok(pubkey) = Pubkey::try_from(info.pubkey.as_slice()) else {
            return;
        };
        if pubkey == CLOCK_SYSVAR {
            return;
        }
        self.advance(account.slot);
        match conn {
            Conn::Pool => {
                let Some(&target) = self.owner.get(&pubkey) else {
                    return;
                };
                self.seq += 1;
                let Some(signature) = &info.txn_signature else {
                    self.stats[target].unsigned += 1;
                    return;
                };
                let id = (account.slot, signature.clone());
                if self.closed.contains_key(&id) {
                    self.late_accounts += 1;
                    return;
                }
                self.supersede(target, &id, now);
                let seq = self.seq;
                let group = self.open.entry(id).or_insert_with(|| Open {
                    last_seq: seq - 1,
                    contiguous: true,
                    opened: now,
                    last_at: now,
                    per_target: BTreeMap::new(),
                });
                group.contiguous &= group.last_seq + 1 == seq;
                group.last_seq = seq;
                group.last_at = now;
                *group.per_target.entry(target).or_default() += 1;
            }
            Conn::Shared => {
                if !self.shared.contains(&pubkey) {
                    return;
                }
                let Some(signature) = &info.txn_signature else {
                    return;
                };
                let id = (account.slot, signature.clone());
                if let Some(terminated) = self.closed.get(&id) {
                    self.shared_after_status += 1;
                    self.shared_skew_us
                        .push(micros(now.duration_since(*terminated)));
                } else if self.open.contains_key(&id) {
                    self.shared_before_status += 1;
                }
            }
        }
    }

    /// Would releasing a pool's open group as soon as another transaction
    /// writes the same pool have released it whole?
    fn supersede(&mut self, target: usize, id: &GroupId, now: Instant) {
        if self.superseded.contains_key(id) {
            self.stats[target].wrong_release += 1;
        }
        if let Some(previous) = self.latest.insert(target, id.clone())
            && previous != *id
            && self.open.contains_key(&previous)
            && !self.superseded.contains_key(&previous)
        {
            self.superseded.insert(previous, now);
            self.stats[target].superseded += 1;
        }
    }

    fn status(&mut self, id: GroupId, now: Instant) {
        self.statuses += 1;
        if let Some(at) = self.superseded.remove(&id)
            && let Some(group) = self.open.get(&id)
        {
            let early = micros(now.duration_since(at));
            for target in group.per_target.keys() {
                self.stats[*target].early_by_us.push(early);
            }
        }
        if self.processed.contains(&id.0) {
            self.statuses_after_processed += 1;
        }
        self.advance(id.0);
        match self.open.remove(&id) {
            None => self.statuses_without_writes += 1,
            Some(group) => {
                let delay = micros(now.duration_since(group.last_at));
                for (target, writes) in group.per_target {
                    let stats = &mut self.stats[target];
                    stats.groups += 1;
                    stats.pool_updates += writes;
                    if writes > 1 {
                        stats.multi += 1;
                        stats.mixed += writes - 1;
                    }
                    if !group.contiguous {
                        stats.noncontiguous += 1;
                    }
                    stats.delays_us.push(delay);
                }
            }
        }
        self.closed.insert(id, now);
    }

    fn advance(&mut self, slot: u64) {
        if slot <= self.max_slot {
            return;
        }
        self.max_slot = slot;
        let floor = slot.saturating_sub(CLOSED_SLOTS_KEPT);
        self.closed.retain(|(slot, _), _| *slot >= floor);
        self.processed = self.processed.split_off(&floor);
    }

    fn sweep(&mut self, now: Instant) {
        let orphan_after = self.orphan_after;
        self.superseded
            .retain(|_, at| now.duration_since(*at) < orphan_after);
        let stats = &mut self.stats;
        self.open.retain(|_, group| {
            let alive = now.duration_since(group.opened) < orphan_after;
            if !alive {
                for target in group.per_target.keys() {
                    stats[*target].orphans += 1;
                }
            }
            alive
        });
    }

    fn findings(mut self, slot_statuses: bool) -> Vec<Finding> {
        let mut by_label: BTreeMap<String, TargetStats> = BTreeMap::new();
        for (label, stats) in self.labels.iter().zip(self.stats.iter_mut()) {
            by_label.entry(label.clone()).or_default().merge(stats);
        }
        let mut findings: Vec<Finding> = by_label
            .into_iter()
            .map(|(label, mut s)| {
                s.delays_us.sort_unstable();
                s.early_by_us.sort_unstable();
                Finding::new(
                    CHECK,
                    s.orphans == 0 && s.unsigned == 0 && s.wrong_release == 0,
                    format!(
                        "{label}: groups={} multi_key={} noncontiguous={} mixed_if_per_update={}/{} ({}) \
                         delay_us p50={} p90={} p99={} max={} orphans={} unsigned={} \
                         superseded={} wrong_release={} early_by_us p50={} p99={} max={}",
                        s.groups,
                        s.multi,
                        s.noncontiguous,
                        s.mixed,
                        s.pool_updates,
                        ratio(s.mixed, s.pool_updates),
                        percentile(&s.delays_us, 50),
                        percentile(&s.delays_us, 90),
                        percentile(&s.delays_us, 99),
                        s.delays_us.last().copied().unwrap_or(0),
                        s.orphans,
                        s.unsigned,
                        s.superseded,
                        s.wrong_release,
                        percentile(&s.early_by_us, 50),
                        percentile(&s.early_by_us, 99),
                        s.early_by_us.last().copied().unwrap_or(0),
                    ),
                )
            })
            .collect();
        findings.push(Finding::new(
            "status after its writes",
            self.late_accounts == 0,
            format!(
                "{} account writes arrived after their txn status",
                self.late_accounts
            ),
        ));
        findings.push(Finding::new(
            "status without pool writes",
            true,
            format!(
                "{} of {} statuses touched pool keys without writing them",
                self.statuses_without_writes, self.statuses
            ),
        ));
        findings.push(if slot_statuses {
            Finding::new(
                "status after processed slot",
                true,
                format!(
                    "{} of {} statuses ({}) arrived after their slot's processed status",
                    self.statuses_after_processed,
                    self.statuses,
                    ratio(self.statuses_after_processed, self.statuses),
                ),
            )
        } else {
            Finding::new(
                "status after processed slot",
                false,
                "not measured: provider sends no slot statuses",
            )
        });
        self.shared_skew_us.sort_unstable();
        findings.push(Finding::new(
            "shared writes vs pool status",
            true,
            format!(
                "before status={} after status={} skew_us p50={} p99={} max={}",
                self.shared_before_status,
                self.shared_after_status,
                percentile(&self.shared_skew_us, 50),
                percentile(&self.shared_skew_us, 99),
                self.shared_skew_us.last().copied().unwrap_or(0),
            ),
        ));
        findings
    }
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn percentile(sorted: &[u64], pct: usize) -> u64 {
    match sorted.len() {
        0 => 0,
        n => sorted[(n - 1) * pct / 100],
    }
}

fn ratio(part: u64, whole: u64) -> String {
    let bp = part.saturating_mul(10_000).checked_div(whole).unwrap_or(0);
    format!("{}.{:02}%", bp / 100, bp % 100)
}
