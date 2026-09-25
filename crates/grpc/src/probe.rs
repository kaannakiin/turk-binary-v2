use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::time::Duration;

use domain::chain::CLOCK_SYSVAR;
use domain::{Commitment, Pubkey};
use futures::{SinkExt, StreamExt};
use tokio::time::{Instant, timeout};
use yellowstone_grpc_proto::prelude::{
    SubscribeRequest, SubscribeRequestFilterAccounts, SubscribeUpdate,
    subscribe_update::UpdateOneof,
};
use yellowstone_grpc_proto::tonic::Status;

use crate::classify::{Failure, classify};
use crate::connector::{Connector, TonicConnector};
use crate::events::{LimitViolation, SlotStatus};
use crate::request::{Heartbeat, Limits, build_request, ping_request};
use crate::settings::SlotSource;
use crate::{GrpcError, GrpcSettings};

const WAIT: Duration = Duration::from_secs(10);
const PROBE_PUBKEYS: usize = 20_000;
const PROBE_FILTERS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    Slots,
    Clock,
    PingOnly,
    Replay,
    Limits,
    SlotBacklog,
}

impl ProbeKind {
    pub const ALL: [Self; 6] = [
        Self::Slots,
        Self::Clock,
        Self::PingOnly,
        Self::Replay,
        Self::Limits,
        Self::SlotBacklog,
    ];
}

impl FromStr for ProbeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "slots" => Ok(Self::Slots),
            "clock" => Ok(Self::Clock),
            "ping-only" => Ok(Self::PingOnly),
            "replay" => Ok(Self::Replay),
            "limits" => Ok(Self::Limits),
            "slot-backlog" => Ok(Self::SlotBacklog),
            other => Err(format!("unknown probe `{other}`")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub check: &'static str,
    pub ok: bool,
    pub detail: String,
}

impl Finding {
    fn new(check: &'static str, ok: bool, detail: impl Into<String>) -> Self {
        Self {
            check,
            ok,
            detail: detail.into(),
        }
    }
}

/// Read-only checks of what the provider supports. They open short-lived
/// subscriptions and never send a transaction.
pub async fn probe(
    endpoint: String,
    x_token: Option<String>,
    settings: &GrpcSettings,
    kinds: &[ProbeKind],
) -> Result<Vec<Finding>, GrpcError> {
    let connector = TonicConnector::new(endpoint, x_token, settings)?;
    let mut findings = Vec::new();
    for kind in kinds {
        match kind {
            ProbeKind::Slots => findings.push(slots(&connector).await),
            ProbeKind::Clock => findings.extend(clock(&connector).await),
            ProbeKind::PingOnly => findings.push(ping_only(&connector).await),
            ProbeKind::Replay => findings.extend(replay(&connector).await),
            ProbeKind::Limits => findings.extend(limits(&connector).await),
            ProbeKind::SlotBacklog => findings.push(slot_backlog(&connector).await),
        }
    }
    Ok(findings)
}

pub async fn resolve_slot_source(
    endpoint: String,
    x_token: Option<String>,
    settings: &GrpcSettings,
) -> Result<SlotSource, GrpcError> {
    if settings.slot_source != SlotSource::Auto {
        return Ok(settings.slot_source);
    }
    let connector = TonicConnector::new(endpoint, x_token, settings)?;
    let finding = slots(&connector).await;
    tracing::info!(accepted = finding.ok, detail = %finding.detail, "grpc slot source probe");
    Ok(if finding.ok {
        SlotSource::Slots
    } else {
        SlotSource::BlocksMeta
    })
}

fn clock_request(heartbeat: Heartbeat, seq: u64, from_slot: Option<u64>) -> SubscribeRequest {
    let limits = Limits {
        pubkeys_per_filter: 100,
        account_filters: None,
        request_bytes: usize::MAX,
    };
    build_request(
        &BTreeMap::new(),
        heartbeat,
        Commitment::Processed,
        &limits,
        seq,
        from_slot,
    )
    .map(|built| built.request)
    .unwrap_or_default()
}

async fn next<S>(stream: &mut S, until: Instant) -> Option<Result<SubscribeUpdate, Status>>
where
    S: futures::Stream<Item = Result<SubscribeUpdate, Status>> + Unpin,
{
    timeout(
        until.saturating_duration_since(Instant::now()),
        stream.next(),
    )
    .await
    .ok()
    .flatten()
}

fn clock_slot(update: &SubscribeUpdate) -> Option<u64> {
    match &update.update_oneof {
        Some(UpdateOneof::Account(a))
            if a.account
                .as_ref()
                .is_some_and(|i| i.pubkey == CLOCK_SYSVAR.to_bytes()) =>
        {
            Some(a.slot)
        }
        _ => None,
    }
}

async fn slots(connector: &TonicConnector) -> Finding {
    let (_sink, mut stream) = match connector
        .subscribe(clock_request(Heartbeat::Slots, 1, None))
        .await
    {
        Ok(opened) => opened,
        Err(status) => return Finding::new("slots filter", false, format!("refused: {status}")),
    };
    let until = Instant::now() + WAIT;
    while let Some(message) = next(&mut stream, until).await {
        match message {
            Ok(SubscribeUpdate {
                update_oneof: Some(UpdateOneof::Slot(slot)),
                ..
            }) => {
                return Finding::new(
                    "slots filter",
                    true,
                    format!("slot updates arrive (slot {})", slot.slot),
                );
            }
            Ok(_) => {}
            Err(status) => {
                return Finding::new("slots filter", false, format!("refused: {status}"));
            }
        }
    }
    Finding::new("slots filter", false, "no slot update within 10 s")
}

/// Reports slot updates for slots older than the first `processed` one
/// the new subscription saw: the slot tree must not treat their parents as
/// the start of the live chain.
async fn slot_backlog(connector: &TonicConnector) -> Finding {
    const CHECK: &str = "slot backlog on connect";
    let (_sink, mut stream) = match connector
        .subscribe(clock_request(Heartbeat::Slots, 1, None))
        .await
    {
        Ok(opened) => opened,
        Err(status) => return Finding::new(CHECK, false, format!("refused: {status}")),
    };
    let until = Instant::now() + Duration::from_secs(6);
    let mut first_processed = None;
    let mut seen = Vec::new();
    while let Some(message) = next(&mut stream, until).await {
        let Ok(SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Slot(slot)),
            ..
        }) = message
        else {
            continue;
        };
        let status = SlotStatus::from(slot.status);
        if status == SlotStatus::Processed && first_processed.is_none() {
            first_processed = Some(slot.slot);
        }
        seen.push((slot.slot, status, slot.parent.is_some(), first_processed));
    }
    let Some(first) = first_processed else {
        return Finding::new(CHECK, false, "no processed slot update within 6 s");
    };
    let older: BTreeMap<String, (usize, u64)> = seen
        .iter()
        .filter(|(slot, ..)| *slot < first)
        .fold(BTreeMap::new(), |mut acc, (slot, status, parent, at)| {
            let when = if at.is_none() { "before" } else { "after" };
            let with = if *parent { "with parent" } else { "no parent" };
            let entry = acc
                .entry(format!("{status:?} {with}, {when} it"))
                .or_insert((0, 0));
            entry.0 += 1;
            entry.1 = entry.1.max(first - slot);
            acc
        });
    let detail = if older.is_empty() {
        format!("first processed slot {first}; no update for an older slot")
    } else {
        let parts: Vec<String> = older
            .iter()
            .map(|(k, (n, back))| format!("{n}x {k} (up to {back} slots back)"))
            .collect();
        format!("first processed slot {first}; older: {}", parts.join("; "))
    };
    Finding::new(CHECK, true, detail)
}

async fn clock(connector: &TonicConnector) -> Vec<Finding> {
    let (_sink, mut stream) = match connector
        .subscribe(clock_request(Heartbeat::Clock, 7, None))
        .await
    {
        Ok(opened) => opened,
        Err(status) => {
            return vec![Finding::new(
                "clock sysvar",
                false,
                format!("refused: {status}"),
            )];
        }
    };
    let until = Instant::now() + Duration::from_secs(5);
    let mut slots = Vec::new();
    let mut tagged = false;
    while let Some(Ok(update)) = next(&mut stream, until).await {
        if let Some(slot) = clock_slot(&update) {
            slots.push(slot);
            tagged |= update.filters.iter().any(|name| name == "a7.0");
        }
    }
    let streamed = slots.len() >= 3 && slots.windows(2).all(|w| w[0] <= w[1]);
    vec![
        Finding::new(
            "clock sysvar",
            streamed,
            format!(
                "{} updates in 5 s, slots {:?}..{:?}",
                slots.len(),
                slots.first(),
                slots.last()
            ),
        ),
        Finding::new(
            "filter name tagging",
            tagged,
            if tagged {
                "updates carry the filter names, filter changes can be confirmed"
            } else {
                "updates do not carry filter names; effective slots fall back to a timeout"
            },
        ),
    ]
}

async fn ping_only(connector: &TonicConnector) -> Finding {
    let (mut sink, mut stream) = match connector
        .subscribe(clock_request(Heartbeat::Clock, 1, None))
        .await
    {
        Ok(opened) => opened,
        Err(status) => {
            return Finding::new("ping-only request", false, format!("refused: {status}"));
        }
    };
    let until = Instant::now() + WAIT;
    while let Some(Ok(update)) = next(&mut stream, until).await {
        if clock_slot(&update).is_some() {
            break;
        }
    }
    if sink.send(ping_request(42)).await.is_err() {
        return Finding::new("ping-only request", false, "could not send the ping");
    }
    let mut ponged = false;
    let until = Instant::now() + Duration::from_secs(5);
    while let Some(Ok(update)) = next(&mut stream, until).await {
        match update.update_oneof {
            Some(UpdateOneof::Pong(pong)) if pong.id == 42 => ponged = true,
            _ if ponged && clock_slot(&update).is_some() => {
                return Finding::new(
                    "ping-only request",
                    true,
                    "answered with a pong, filters kept",
                );
            }
            _ => {}
        }
    }
    let detail = if ponged {
        "the stream stopped after the ping: a ping-only request wipes the filters here"
    } else {
        "no pong within 5 s"
    };
    Finding::new("ping-only request", false, detail)
}

async fn replay(connector: &TonicConnector) -> Vec<Finding> {
    let first = match connector.replay_info().await {
        Ok(Some(first)) => first,
        Ok(None) => {
            return vec![Finding::new(
                "replay",
                false,
                "server reports no replay support",
            )];
        }
        Err(status) => {
            return vec![Finding::new(
                "replay",
                false,
                format!("replay info refused: {status}"),
            )];
        }
    };
    let mut findings = vec![Finding::new(
        "replay window",
        true,
        format!("first available slot {first}"),
    )];
    let (_sink, mut stream) = match connector
        .subscribe(clock_request(Heartbeat::Clock, 1, None))
        .await
    {
        Ok(opened) => opened,
        Err(status) => return vec![Finding::new("replay", false, format!("refused: {status}"))],
    };
    let tip = loop {
        match next(&mut stream, Instant::now() + WAIT).await {
            Some(Ok(update)) => {
                if let Some(slot) = clock_slot(&update) {
                    break slot;
                }
            }
            _ => {
                return vec![Finding::new(
                    "replay",
                    false,
                    "no clock update to anchor the replay",
                )];
            }
        }
    };
    let from = tip.saturating_sub(20).max(first);
    findings.push(
        match connector
            .subscribe(clock_request(Heartbeat::Clock, 1, Some(from)))
            .await
        {
            Ok((_sink, mut replayed)) => match next(&mut replayed, Instant::now() + WAIT).await {
                Some(Ok(update)) => {
                    let slot = clock_slot(&update).unwrap_or(0);
                    Finding::new(
                        "replay from a recent slot",
                        slot <= from + 8,
                        format!("asked {from}, first clock {slot}"),
                    )
                }
                Some(Err(status)) => Finding::new(
                    "replay from a recent slot",
                    false,
                    format!("refused: {status}"),
                ),
                None => Finding::new("replay from a recent slot", false, "nothing within 10 s"),
            },
            Err(status) => Finding::new(
                "replay from a recent slot",
                false,
                format!("refused: {status}"),
            ),
        },
    );
    let out_of_range = match connector
        .subscribe(clock_request(Heartbeat::Clock, 1, Some(1)))
        .await
    {
        Ok((_sink, mut stream)) => match next(&mut stream, Instant::now() + WAIT).await {
            Some(Err(status)) => Some(status),
            _ => None,
        },
        Err(status) => Some(status),
    };
    findings.push(match out_of_range {
        Some(status) => Finding::new(
            "replay out of range",
            classify(&status) == Failure::ReplayOutOfRange,
            format!("{:?}: {}", status.code(), status.message()),
        ),
        None => Finding::new(
            "replay out of range",
            false,
            "from_slot = 1 was not refused",
        ),
    });
    findings
}

async fn limits(connector: &TonicConnector) -> Vec<Finding> {
    let pubkeys = |n: usize| -> Vec<String> {
        std::iter::once(CLOCK_SYSVAR.to_string())
            .chain((1..n).map(|_| Pubkey::new_unique().to_string()))
            .collect()
    };
    let one_filter = SubscribeRequest {
        accounts: HashMap::from([(
            "limit".to_owned(),
            SubscribeRequestFilterAccounts {
                account: pubkeys(PROBE_PUBKEYS),
                ..SubscribeRequestFilterAccounts::default()
            },
        )]),
        ..SubscribeRequest::default()
    };
    let many_filters = SubscribeRequest {
        accounts: pubkeys(PROBE_FILTERS)
            .into_iter()
            .enumerate()
            .map(|(i, key)| {
                (
                    format!("limit{i}"),
                    SubscribeRequestFilterAccounts {
                        account: vec![key],
                        ..SubscribeRequestFilterAccounts::default()
                    },
                )
            })
            .collect(),
        ..SubscribeRequest::default()
    };
    vec![
        limit_finding(
            "pubkeys per filter",
            PROBE_PUBKEYS,
            attempt(connector, one_filter).await,
        ),
        limit_finding(
            "account filters",
            PROBE_FILTERS,
            attempt(connector, many_filters).await,
        ),
    ]
}

async fn attempt(connector: &TonicConnector, request: SubscribeRequest) -> Result<(), Status> {
    let (_sink, mut stream) = connector.subscribe(request).await?;
    match next(&mut stream, Instant::now() + WAIT).await {
        Some(Err(status)) => Err(status),
        _ => Ok(()),
    }
}

fn limit_finding(check: &'static str, tried: usize, result: Result<(), Status>) -> Finding {
    match result {
        Ok(()) => Finding::new(check, true, format!("at least {tried}")),
        Err(status) => match classify(&status) {
            Failure::Limit(
                LimitViolation::Pubkeys { limit } | LimitViolation::Filters { limit },
            ) => Finding::new(check, true, format!("limit {limit}")),
            _ => Finding::new(check, false, format!("refused: {status}")),
        },
    }
}
