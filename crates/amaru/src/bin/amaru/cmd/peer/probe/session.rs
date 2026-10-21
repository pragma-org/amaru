// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use amaru_kernel::{NetworkName, Peer, Point};
use amaru_network::connection::TokioConnections;
use amaru_ouroboros::{
    ConnectionProvider, ConnectionsResource, PeerTrackingResource, in_memory_chain_store::InMemoryChainStore,
};
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, InitiatorMessage as CsLocal, InitiatorResult as CsResult},
    deserializers,
    handshake::{self, HandshakeResult, RefuseReason},
    keepalive::{InitiatorMessage as KaLocal, InitiatorResult as KaResult, register_keepalive_oneshot},
    mux::{self, MuxMessage},
    peer_sharing::{ShareResult, register_peer_sharing_initiator_oneshot},
    peer_tracking::InMemoryPeerTracking,
    protocol::{
        Inputs, PROTO_HANDSHAKE, PROTO_N2N_CHAIN_SYNC, PROTO_N2N_KEEP_ALIVE, PROTO_N2N_PEER_SHARE, Role,
        ingress_limit,
    },
    protocol_messages::{version_data::PeerSharing, version_number::VersionNumber, version_table::VersionTable},
    store_effects::ResourceHeaderStore,
};
use amaru_pure_stage::{Effects, StageGraph, StageRef, Void, tokio::TokioBuilder};
use anyhow::{Context, bail};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::runtime::Handle;

#[derive(Debug, Clone)]
pub struct SessionRequest {
    pub peer: Peer,
    pub network: NetworkName,
    /// Wall-clock budget from TCP connect start until this attempt finishes (`--timeout`).
    pub session_timeout: Duration,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
    pub protocol_timeout: Duration,
    pub want_peershare: bool,
    pub want_tip: bool,
    /// Keep-alive pings after Accept (`count`, `interval`). `None` skips ping.
    pub want_ping: Option<(u32, Duration)>,
    /// When set, each keep-alive RTT (milliseconds) is sent as soon as it is measured.
    pub ping_progress: Option<tokio::sync::mpsc::UnboundedSender<f64>>,
    pub want_timetrack: bool,
    /// Continue an existing timeline (e.g. after a prior query handshake). `(started, last)`.
    pub timetrack_continue: Option<(Instant, Instant)>,
    /// Highest N2N version to propose (V11..=this, clipped to [`VersionNumber::SUPPORTED`]).
    pub max_n2n_version: VersionNumber,
    pub peer_share_amount: u8,
}

/// One probe timeline step (`--timetrack`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TimeTrackStep {
    /// Stable step id, e.g. `tcp_connect_ok`, `handshake_ok`, `tip_ok`.
    pub step: String,
    /// Wall-clock instant (UTC, millisecond precision).
    pub at: String,
    /// Milliseconds since the previous step (0 for the first).
    pub delta_ms: u64,
    /// Milliseconds since timeline start.
    pub elapsed_ms: u64,
    /// Cumulative mux/application bytes sent on this TCP connection.
    pub bytes_sent: u64,
    /// Cumulative mux/application bytes received on this TCP connection.
    pub bytes_recv: u64,
}

#[derive(Clone)]
struct TimingCtx {
    started: Instant,
    last: Instant,
    connections: Arc<TokioConnections>,
    conn_id: amaru_ouroboros::ConnectionId,
}

impl std::fmt::Debug for TimingCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimingCtx")
            .field("started", &self.started)
            .field("last", &self.last)
            .field("conn_id", &self.conn_id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for TimingCtx {
    fn eq(&self, _other: &Self) -> bool {
        // Stage equality ignores live clocks / connection handles.
        true
    }
}

/// Live ping RTT notifications for human streaming output (`#[serde(skip)]` on [`Driver`]).
#[derive(Clone)]
struct PingProgressTx(tokio::sync::mpsc::UnboundedSender<f64>);

impl std::fmt::Debug for PingProgressTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PingProgressTx")
    }
}

impl PartialEq for PingProgressTx {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl TimingCtx {
    fn mark(&mut self, step: &str) -> TimeTrackStep {
        let now = Instant::now();
        let delta_ms = now.duration_since(self.last).as_millis() as u64;
        let elapsed_ms = now.duration_since(self.started).as_millis() as u64;
        self.last = now;
        let (bytes_sent, bytes_recv) = self.connections.byte_counts(self.conn_id).unwrap_or((0, 0));
        TimeTrackStep {
            step: step.to_string(),
            at: format_wall_clock(SystemTime::now()),
            delta_ms,
            elapsed_ms,
            bytes_sent,
            bytes_recv,
        }
    }
}

fn format_wall_clock(t: SystemTime) -> String {
    let Ok(dur) = t.duration_since(UNIX_EPOCH) else {
        return "1970-01-01T00:00:00.000Z".to_string();
    };
    let secs = dur.as_secs();
    let millis = dur.subsec_millis();
    let days = secs / 86400;
    let day_secs = secs % 86400;
    let hours = day_secs / 3600;
    let minutes = (day_secs % 3600) / 60;
    let seconds = day_secs % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

/// Days since Unix epoch → proleptic Gregorian Y-M-D (Howard Hinnant algorithm).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// Record a timeline step that is not tied to an open mux connection (e.g. ping).
pub fn mark_standalone_step(
    started: Instant,
    last: &mut Instant,
    step: &str,
    bytes_sent: u64,
    bytes_recv: u64,
) -> TimeTrackStep {
    let now = Instant::now();
    let delta_ms = now.duration_since(*last).as_millis() as u64;
    let elapsed_ms = now.duration_since(started).as_millis() as u64;
    *last = now;
    TimeTrackStep {
        step: step.to_string(),
        at: format_wall_clock(SystemTime::now()),
        delta_ms,
        elapsed_ms,
        bytes_sent,
        bytes_recv,
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct HandshakeInfo {
    /// All N2N versions the remote listed in `MsgQueryReply` (from `--handshake` query).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offered_versions: Vec<u64>,
    /// Negotiated version from Accept (session / keep-alive path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_magic: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiator_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_sharing: Option<bool>,
    /// Negotiated VersionData `query` flag. Session handshakes always negotiate `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TipInfo {
    pub slot: u64,
    pub block_height: u64,
    pub hash: String,
}

impl TipInfo {
    pub fn from_point(point: Point) -> Self {
        match point {
            Point::Origin => Self { slot: 0, block_height: 0, hash: "origin".to_string() },
            Point::Specific(slot, hash, height) => {
                Self { slot: slot.as_u64(), block_height: height.as_u64(), hash: hash.to_string() }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProbeReport {
    /// Target as given on the CLI (`IP:PORT`, `HOSTNAME:PORT`, or CIP-0155 SRV DNS name).
    pub address: String,
    /// Dialed socket after A/AAAA or CIP-0155 SRV resolution; omitted when already a socket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<String>,
    pub network: String,
    /// Present only when ping samples were collected (`--ping` / `--all`).
    /// Values are milliseconds (fractional; keep-alive RTT can be sub-ms on LAN).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ping_rtts_ms: Vec<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ping_interval_ms: Option<u64>,
    /// Negotiated VersionData; present after a successful handshake.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handshake: Option<HandshakeInfo>,
    /// Peer-sharing result; omitted when that action was not requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers: Option<Vec<String>>,
    /// Chain-sync tip; omitted when that action was not requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tip: Option<TipInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peer_checks: Vec<PeerCheck>,
    /// Per-step wall clock / deltas / byte counters when `--timetrack` is set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timetrack: Vec<TimeTrackStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Stage at which a `--peer-check` attempt failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PsCheckFailedAt {
    Tcp,
    Handshake,
    Ping,
    Tip,
    PeerShare,
}

impl PsCheckFailedAt {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Handshake => "handshake",
            Self::Ping => "ping",
            Self::Tip => "tip",
            Self::PeerShare => "peer_share",
        }
    }
}

impl std::fmt::Display for PsCheckFailedAt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classify a probe session / connect error message into a failure stage.
pub fn classify_failure(message: &str) -> PsCheckFailedAt {
    let lower = message.to_ascii_lowercase();
    if lower.contains("tcp connect") || lower.contains("invalid peer address") {
        return PsCheckFailedAt::Tcp;
    }
    if lower.contains("keep-alive") || lower.contains("ping timed out") || lower.contains("during keep-alive") {
        return PsCheckFailedAt::Ping;
    }
    if lower.contains("tip timed out") || lower.contains("during tip") || lower.contains("chainsync") {
        return PsCheckFailedAt::Tip;
    }
    if lower.contains("peershare") || lower.contains("peer-share") || lower.contains("peer_share") {
        return PsCheckFailedAt::PeerShare;
    }
    // Handshake refuse/timeout/mux, or outer session timeout with no finer hint.
    PsCheckFailedAt::Handshake
}

/// Which post-handshake mini-protocols a peer-check should run (on the same TCP session).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PsCheckExtras {
    pub ping: bool,
    pub tip: bool,
    pub peer_share: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerCheck {
    pub address: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<PsCheckFailedAt>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negotiated_version: Option<u64>,
    /// Keep-alive RTTs in milliseconds when ping was requested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ping_rtts_ms: Vec<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tip: Option<TipInfo>,
    /// Peer-share addresses from this check only (never recursively peer-checked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timetrack: Vec<TimeTrackStep>,
}

impl PeerCheck {
    pub fn hard_failure(address: impl Into<String>, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let failed_at = classify_failure(&detail);
        Self {
            address: address.into(),
            ok: false,
            failed_at: Some(failed_at),
            detail,
            negotiated_version: None,
            ping_rtts_ms: Vec::new(),
            tip: None,
            peers: None,
            timetrack: Vec::new(),
        }
    }

    /// Build a check result from a completed Accept-mode session.
    pub fn from_session(address: impl Into<String>, partial: SessionPartial, extras: PsCheckExtras) -> Self {
        let address = address.into();
        let negotiated_version = partial.handshake.as_ref().and_then(|h| h.version);
        let handshake_ok = negotiated_version.is_some();

        if let Some(err) = partial.errors.first() {
            let failed_at = classify_failure(err);
            return Self {
                address,
                ok: false,
                failed_at: Some(failed_at),
                detail: err.clone(),
                negotiated_version,
                ping_rtts_ms: partial.ping_rtts_ms,
                tip: partial.tip,
                peers: partial.peers,
                timetrack: partial.timetrack,
            };
        }

        if !handshake_ok {
            return Self {
                address,
                ok: false,
                failed_at: Some(PsCheckFailedAt::Handshake),
                detail: "handshake did not Accept".to_string(),
                negotiated_version: None,
                ping_rtts_ms: partial.ping_rtts_ms,
                tip: partial.tip,
                peers: partial.peers,
                timetrack: partial.timetrack,
            };
        }

        // Requested extras must produce their results (empty peer-share list is still success).
        if extras.ping && partial.ping_rtts_ms.is_empty() {
            return Self {
                address,
                ok: false,
                failed_at: Some(PsCheckFailedAt::Ping),
                detail: "no keep-alive ping response".to_string(),
                negotiated_version,
                ping_rtts_ms: partial.ping_rtts_ms,
                tip: partial.tip,
                peers: partial.peers,
                timetrack: partial.timetrack,
            };
        }
        if extras.tip && partial.tip.is_none() {
            return Self {
                address,
                ok: false,
                failed_at: Some(PsCheckFailedAt::Tip),
                detail: "no tip".to_string(),
                negotiated_version,
                ping_rtts_ms: partial.ping_rtts_ms,
                tip: None,
                peers: partial.peers,
                timetrack: partial.timetrack,
            };
        }
        if extras.peer_share && partial.peers.is_none() {
            return Self {
                address,
                ok: false,
                failed_at: Some(PsCheckFailedAt::PeerShare),
                detail: "peer-share not negotiated or no reply".to_string(),
                negotiated_version,
                ping_rtts_ms: partial.ping_rtts_ms,
                tip: partial.tip,
                peers: None,
                timetrack: partial.timetrack,
            };
        }

        Self {
            address,
            ok: true,
            failed_at: None,
            detail: "handshake ok".to_string(),
            negotiated_version,
            ping_rtts_ms: partial.ping_rtts_ms,
            tip: partial.tip,
            peers: partial.peers,
            timetrack: partial.timetrack,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct SessionPartial {
    pub handshake: Option<HandshakeInfo>,
    pub peers: Option<Vec<String>>,
    pub tip: Option<TipInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ping_rtts_ms: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timetrack: Vec<TimeTrackStep>,
    pub errors: Vec<String>,
}

fn offered_versions_from_table(
    table: &VersionTable<amaru_protocols::protocol_messages::version_data::VersionData>,
) -> Vec<u64> {
    let mut versions: Vec<u64> = table.values.keys().chain(table.unknown.keys()).map(|v| v.as_u64()).collect();
    versions.sort_unstable();
    versions.dedup();
    versions
}

/// Handshake **query** (`query = true`): list every N2N version the remote speaks, then hang up.
pub async fn run_version_query(
    peer: Peer,
    network: NetworkName,
    session_timeout: Duration,
    connect_timeout: Duration,
    handshake_timeout: Duration,
    max_n2n_version: VersionNumber,
    want_timetrack: bool,
    timetrack_continue: Option<(Instant, Instant)>,
) -> anyhow::Result<(Vec<u64>, Vec<TimeTrackStep>)> {
    let req = SessionRequest {
        peer,
        network,
        session_timeout,
        connect_timeout,
        handshake_timeout,
        protocol_timeout: handshake_timeout,
        want_peershare: false,
        want_tip: false,
        want_ping: None,
        ping_progress: None,
        want_timetrack,
        timetrack_continue,
        max_n2n_version,
        peer_share_amount: 10,
    };
    let partial = run_session_inner(req, HandshakeMode::Query).await?;
    let offered = partial.handshake.as_ref().map(|h| h.offered_versions.clone()).unwrap_or_default();
    if offered.is_empty() && !partial.errors.is_empty() {
        bail!(partial.errors.join("; "));
    }
    if offered.is_empty() {
        bail!("handshake query returned no versions");
    }
    Ok((offered, partial.timetrack))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum HandshakeMode {
    /// Propose with `query = true`; expect `MsgQueryReply` and stop.
    Query,
    /// Propose session versions; expect `MsgAcceptVersion` then run requested mini-protocols.
    Accept,
}

pub async fn run_session(req: SessionRequest) -> anyhow::Result<SessionPartial> {
    run_session_inner(req, HandshakeMode::Accept).await
}

async fn run_session_inner(req: SessionRequest, handshake_mode: HandshakeMode) -> anyhow::Result<SessionPartial> {
    let session_timeout = req.session_timeout;
    let attempt_started = Instant::now();
    match tokio::time::timeout(session_timeout, run_session_attempt(req, handshake_mode, attempt_started)).await {
        Ok(result) => result,
        Err(_) => {
            bail!("probe timed out after {}", amaru_kernel::utils::duration::format(&session_timeout))
        }
    }
}

async fn run_session_attempt(
    req: SessionRequest,
    handshake_mode: HandshakeMode,
    attempt_started: Instant,
) -> anyhow::Result<SessionPartial> {
    let _guards = deserializers::register_deserializers();
    let connections = Arc::new(TokioConnections::new(65535));

    // Connect may not consume more than the remaining session budget.
    let connect_budget = req.connect_timeout.min(req.session_timeout.saturating_sub(attempt_started.elapsed()));
    if connect_budget.is_zero() {
        bail!("probe timed out after {}", amaru_kernel::utils::duration::format(&req.session_timeout));
    }

    let (conn_id, pre_steps, timing_for_driver) = if req.want_timetrack {
        let (started, mut last) = req.timetrack_continue.unwrap_or_else(|| {
            let now = Instant::now();
            (now, now)
        });
        let mut steps = vec![mark_standalone_step(started, &mut last, "tcp_connect_start", 0, 0)];
        let conn_id = connections.connect(req.peer, connect_budget).await.context("TCP connect for session")?;
        let (sent, recv) = connections.byte_counts(conn_id).unwrap_or((0, 0));
        steps.push(mark_standalone_step(started, &mut last, "tcp_connect_ok", sent, recv));
        let ctx = TimingCtx { started, last, connections: connections.clone(), conn_id };
        (conn_id, steps, Some(ctx))
    } else {
        let conn_id = connections.connect(req.peer, connect_budget).await.context("TCP connect for session")?;
        (conn_id, Vec::new(), None)
    };

    let mut network = TokioBuilder::default();
    network.resources().put::<ConnectionsResource>(connections.clone());
    network.resources().put::<ResourceHeaderStore>(Arc::new(InMemoryChainStore::new()) as ResourceHeaderStore);
    // Keep-alive (and peer-share) record via PeerTrack; probe has no real tracker.
    network.resources().put::<PeerTrackingResource>(Arc::new(InMemoryPeerTracking::new()) as PeerTrackingResource);

    let (report_out, mut report_rx) = network.output::<SessionPartial>("probe_report", 4);
    let driver = network.stage("probe_driver", driver_stage);
    let driver = network.wire_up(
        driver,
        Driver {
            peer: req.peer,
            conn_id,
            magic: req.network.to_network_magic(),
            handshake_mode,
            want_peershare: req.want_peershare,
            want_tip: req.want_tip,
            want_ping: req.want_ping,
            peer_share_amount: req.peer_share_amount,
            max_n2n_version: req.max_n2n_version,
            protocol_timeout: req.protocol_timeout,
            handshake_timeout: req.handshake_timeout,
            report_to: report_out,
            muxer: None,
            keepalive: None,
            phase: Phase::Start,
            pings_remaining: 0,
            partial: SessionPartial { timetrack: pre_steps, ..SessionPartial::default() },
            timing: timing_for_driver,
            ping_progress: req.ping_progress.map(PingProgressTx),
        },
    );

    if network.preload(&driver, [DriverMsg::Start]).is_err() {
        bail!("preload Start failed");
    }

    let handle = Handle::current();
    let running = network.run(handle);
    // Abort the graph if this attempt is cancelled (session `--timeout`) or returns early.
    struct AbortOnDrop(Option<amaru_pure_stage::tokio::TokioRunning>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            if let Some(running) = self.0.take() {
                running.abort();
            }
        }
    }
    let mut running = AbortOnDrop(Some(running));

    // Outer `--timeout` is the deadline; wait for the report until that future is cancelled.
    let partial = match report_rx.next().await {
        Some(report) => report,
        None => bail!("probe session ended without a report (graph shut down before result delivery)"),
    };

    if let Some(running) = running.0.take() {
        running.abort();
    }
    let _ = connections.close(conn_id).await;
    Ok(partial)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Phase {
    Start,
    Handshaking,
    Ping,
    Tip,
    PeerShare,
    Done,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Driver {
    peer: Peer,
    conn_id: amaru_ouroboros::ConnectionId,
    magic: amaru_kernel::NetworkMagic,
    handshake_mode: HandshakeMode,
    want_peershare: bool,
    want_tip: bool,
    want_ping: Option<(u32, Duration)>,
    peer_share_amount: u8,
    max_n2n_version: VersionNumber,
    protocol_timeout: Duration,
    handshake_timeout: Duration,
    report_to: StageRef<SessionPartial>,
    muxer: Option<StageRef<MuxMessage>>,
    keepalive: Option<StageRef<KaLocal>>,
    phase: Phase,
    /// Remaining keep-alive sends after the in-flight one completes.
    pings_remaining: u32,
    partial: SessionPartial,
    #[serde(skip)]
    timing: Option<TimingCtx>,
    #[serde(skip)]
    ping_progress: Option<PingProgressTx>,
}

fn mark(state: &mut Driver, step: &str) {
    if let Some(timing) = state.timing.as_mut() {
        let entry = timing.mark(step);
        state.partial.timetrack.push(entry);
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum DriverMsg {
    Start,
    Handshake(HandshakeResult),
    Ping(KaResult),
    PingAgain,
    Tip(ChainSyncInitiatorMsg),
    Share(ShareResult),
    /// Mini-protocol child exited (often expected after Done).
    ChildDied,
    /// Mux exited: connection closed, decode failure, or unknown protocol.
    MuxDied,
    StepTimeout,
}

async fn driver_stage(mut state: Driver, msg: DriverMsg, eff: Effects<DriverMsg>) -> Driver {
    // After the report is sent, ignore further mailbox traffic until the outer session aborts.
    if state.phase == Phase::Done {
        return state;
    }

    match msg {
        DriverMsg::Start => {
            mark(&mut state, "handshake_start");
            let muxer = eff.stage("mux", mux::stage).await;
            let muxer = eff.supervise(muxer, DriverMsg::MuxDied);
            // Hold inbound SDUs for every mini-protocol this probe may register later.
            let early = [
                (PROTO_HANDSHAKE.erase(), ingress_limit(PROTO_HANDSHAKE)),
                (PROTO_N2N_CHAIN_SYNC.erase(), ingress_limit(PROTO_N2N_CHAIN_SYNC)),
                (PROTO_N2N_KEEP_ALIVE.erase(), ingress_limit(PROTO_N2N_KEEP_ALIVE)),
                (PROTO_N2N_PEER_SHARE.erase(), ingress_limit(PROTO_N2N_PEER_SHARE)),
            ];
            let muxer = eff
                .wire_up(muxer, mux::State::new(state.conn_id, &early, Role::Initiator, state.peer))
                .await;

            let versions = match state.handshake_mode {
                HandshakeMode::Query => VersionTable::query_through(state.max_n2n_version, state.magic),
                HandshakeMode::Accept => VersionTable::v11_through(state.max_n2n_version, state.magic, true, true),
            };

            let hs_reply = eff.me_ref().contramap(DriverMsg::Handshake);
            let hs = eff.stage("handshake", handshake::initiator()).await;
            let hs = eff.supervise(hs, DriverMsg::ChildDied);
            let hs = eff.wire_up(hs, handshake::HandshakeInitiator::new(muxer.clone(), hs_reply, versions)).await;

            eff.send(
                &muxer,
                MuxMessage::Register {
                    protocol: PROTO_HANDSHAKE.erase(),
                    frame: mux::Frame::OneCborItem,
                    handler: hs.contramap(Inputs::<Void>::Network),
                    max_buffer: ingress_limit(PROTO_HANDSHAKE),
                },
            )
            .await;

            state.muxer = Some(muxer);
            state.phase = Phase::Handshaking;
            eff.set_timeout_at(1, state.handshake_timeout, DriverMsg::StepTimeout).await;
            state
        }
        DriverMsg::Handshake(result) => {
            eff.clear_timeout_at(1).await;
            match result {
                HandshakeResult::Accepted(version, data) => {
                    if state.handshake_mode == HandshakeMode::Query {
                        mark(&mut state, "handshake_unexpected_accept");
                        state.partial.errors.push("handshake query expected MsgQueryReply, got Accept".to_string());
                        return finish(state, &eff).await;
                    }
                    mark(&mut state, "handshake_ok");
                    let mut hs = state.partial.handshake.take().unwrap_or_default();
                    hs.version = Some(version.as_u64());
                    hs.network_magic = Some(data.network_magic().as_u64());
                    hs.initiator_only = Some(data.initiator_only_diffusion_mode());
                    hs.peer_sharing = Some(bool::from(data.peer_sharing()));
                    hs.query = Some(data.query());
                    state.partial.handshake = Some(hs);
                    return advance_after_handshake(state, version, data.peer_sharing(), &eff).await;
                }
                HandshakeResult::Refused(reason) => {
                    mark(&mut state, "handshake_refused");
                    state.partial.errors.push(format!("handshake refused: {}", format_refuse(&reason)));
                    return finish(state, &eff).await;
                }
                HandshakeResult::Query(table) => {
                    mark(&mut state, "handshake_query_ok");
                    let offered = offered_versions_from_table(&table);
                    let mut hs = state.partial.handshake.take().unwrap_or_default();
                    hs.offered_versions = offered;
                    // Query replies include VersionData; surface peer_sharing of the highest known version.
                    if let Some((_, data)) = table.values.iter().next_back() {
                        hs.network_magic = Some(data.network_magic().as_u64());
                        hs.initiator_only = Some(data.initiator_only_diffusion_mode());
                        hs.peer_sharing = Some(bool::from(data.peer_sharing()));
                        hs.query = Some(true);
                    }
                    state.partial.handshake = Some(hs);
                    return finish(state, &eff).await;
                }
            }
        }
        DriverMsg::Ping(result) => {
            eff.clear_timeout_at(5).await;
            match result {
                KaResult::Response { round_trip: Some(rtt), .. } => {
                    let rtt_ms = rtt.as_secs_f64() * 1_000.0;
                    state.partial.ping_rtts_ms.push(rtt_ms);
                    if let Some(tx) = &state.ping_progress {
                        let _ = tx.0.send(rtt_ms);
                    }
                    mark(&mut state, "ping_ok");
                }
                KaResult::Bootstrap | KaResult::Response { round_trip: None, .. } => {
                    return state;
                }
            }
            if state.pings_remaining > 0 {
                let interval = state.want_ping.map(|(_, i)| i).unwrap_or(Duration::from_secs(1));
                state.pings_remaining -= 1;
                eff.schedule_after(DriverMsg::PingAgain, interval).await;
                return state;
            }
            if let Some(ka) = state.keepalive.take() {
                eff.send(&ka, KaLocal::Close).await;
            }
            advance_after_ping(state, &eff).await
        }
        DriverMsg::PingAgain => {
            if let Some(ka) = state.keepalive.clone() {
                mark(&mut state, "ping_start");
                eff.send(&ka, KaLocal::SendKeepAlive).await;
                eff.set_timeout_at(5, state.protocol_timeout, DriverMsg::StepTimeout).await;
            }
            state
        }
        DriverMsg::Tip(msg) => match msg.msg {
            CsResult::Initialize => state,
            CsResult::IntersectFound(_, tip) | CsResult::IntersectNotFound(tip) => {
                state.partial.tip = Some(TipInfo::from_point(tip));
                mark(&mut state, "tip_ok");
                eff.clear_timeout_at(2).await;
                eff.send(&msg.handler, CsLocal::Done).await;
                advance_after_tip(state, &eff).await
            }
            CsResult::RollForward(_, tip) | CsResult::RollBackward(_, tip) => {
                if state.partial.tip.is_none() {
                    state.partial.tip = Some(TipInfo::from_point(tip));
                }
                state
            }
            CsResult::Terminated => {
                if state.partial.tip.is_none() {
                    state.partial.errors.push("chainsync terminated before tip".to_string());
                    mark(&mut state, "tip_failed");
                } else {
                    mark(&mut state, "tip_ok");
                }
                eff.clear_timeout_at(2).await;
                advance_after_tip(state, &eff).await
            }
        },
        DriverMsg::Share(share) => {
            eff.clear_timeout_at(3).await;
            state.partial.peers = Some(share.peers.iter().map(|a| a.to_string()).collect());
            mark(&mut state, "peershare_ok");
            advance_after_peershare(state, &eff).await
        }
        DriverMsg::ChildDied => state,
        DriverMsg::MuxDied => {
            let hint = match state.phase {
                Phase::Handshaking => "mux exited during handshake",
                Phase::Ping => "mux exited during keep-alive ping",
                Phase::Tip => "mux exited during tip",
                Phase::PeerShare => "mux exited during peershare",
                Phase::Start | Phase::Done => "mux exited",
            };
            let died_step = match state.phase {
                Phase::Handshaking => "handshake_mux_died",
                Phase::Ping => "ping_mux_died",
                Phase::Tip => "tip_mux_died",
                Phase::PeerShare => "peershare_mux_died",
                Phase::Start | Phase::Done => "mux_died",
            };
            mark(&mut state, died_step);
            state.partial.errors.push(hint.to_string());
            finish(state, &eff).await
        }
        DriverMsg::StepTimeout => {
            let (label, step) = match state.phase {
                Phase::Start => ("start", "start_timeout"),
                Phase::Handshaking => ("handshake", "handshake_timeout"),
                Phase::Ping => ("keep-alive ping", "ping_timeout"),
                Phase::Tip => ("tip", "tip_timeout"),
                Phase::PeerShare => ("peershare", "peershare_timeout"),
                Phase::Done => ("done", "done_timeout"),
            };
            mark(&mut state, step);
            state.partial.errors.push(format!("{label} timed out"));
            finish(state, &eff).await
        }
    }
}

async fn advance_after_handshake(
    mut state: Driver,
    _version: VersionNumber,
    peer_sharing: PeerSharing,
    eff: &Effects<DriverMsg>,
) -> Driver {
    let Some(muxer) = state.muxer.clone() else {
        state.partial.errors.push("internal: muxer missing after handshake".to_string());
        return finish(state, eff).await;
    };

    if let Some((count, _)) = state.want_ping {
        return start_ping(state, muxer, count, eff).await;
    }

    if state.want_tip {
        return start_tip(state, muxer, eff).await;
    }

    if state.want_peershare {
        return start_peershare(state, muxer, peer_sharing, eff).await;
    }

    finish(state, eff).await
}

async fn start_ping(mut state: Driver, muxer: StageRef<MuxMessage>, count: u32, eff: &Effects<DriverMsg>) -> Driver {
    mark(&mut state, "ping_start");
    state.phase = Phase::Ping;
    state.pings_remaining = count.saturating_sub(1);
    let report_to = eff.me_ref().contramap(DriverMsg::Ping);
    let ka = register_keepalive_oneshot(state.peer, state.conn_id, &muxer, report_to, eff, DriverMsg::ChildDied).await;
    state.keepalive = Some(ka.clone());
    eff.send(&ka, KaLocal::SendKeepAlive).await;
    eff.set_timeout_at(5, state.protocol_timeout, DriverMsg::StepTimeout).await;
    state
}

async fn advance_after_ping(state: Driver, eff: &Effects<DriverMsg>) -> Driver {
    let Some(muxer) = state.muxer.clone() else {
        return finish(state, eff).await;
    };
    let peer_sharing = state
        .partial
        .handshake
        .as_ref()
        .and_then(|h| h.peer_sharing)
        .map(|enabled| if enabled { PeerSharing::Enabled } else { PeerSharing::Disabled })
        .unwrap_or(PeerSharing::Disabled);

    if state.want_tip {
        return start_tip(state, muxer, eff).await;
    }
    if state.want_peershare {
        return start_peershare(state, muxer, peer_sharing, eff).await;
    }
    finish(state, eff).await
}

async fn start_tip(mut state: Driver, muxer: StageRef<MuxMessage>, eff: &Effects<DriverMsg>) -> Driver {
    mark(&mut state, "tip_start");
    state.phase = Phase::Tip;
    let pipeline = eff.me_ref().contramap(DriverMsg::Tip);
    let _ =
        chainsync::register_chainsync_initiator(&muxer, state.peer, state.conn_id, pipeline, eff, DriverMsg::ChildDied)
            .await;
    eff.set_timeout_at(2, state.protocol_timeout, DriverMsg::StepTimeout).await;
    state
}

async fn advance_after_tip(state: Driver, eff: &Effects<DriverMsg>) -> Driver {
    let Some(muxer) = state.muxer.clone() else {
        return finish(state, eff).await;
    };
    let peer_sharing = state
        .partial
        .handshake
        .as_ref()
        .and_then(|h| h.peer_sharing)
        .map(|enabled| if enabled { PeerSharing::Enabled } else { PeerSharing::Disabled })
        .unwrap_or(PeerSharing::Disabled);
    if state.want_peershare {
        return start_peershare(state, muxer, peer_sharing, eff).await;
    }
    finish(state, eff).await
}

async fn advance_after_peershare(state: Driver, eff: &Effects<DriverMsg>) -> Driver {
    finish(state, eff).await
}

async fn start_peershare(
    mut state: Driver,
    muxer: StageRef<MuxMessage>,
    peer_sharing: PeerSharing,
    eff: &Effects<DriverMsg>,
) -> Driver {
    if peer_sharing != PeerSharing::Enabled {
        // Soft-skip: do not try the mini-protocol and do not treat as probe failure.
        mark(&mut state, "peershare_skipped");
        state.partial.peers = Some(Vec::new());
        return finish(state, eff).await;
    }

    mark(&mut state, "peershare_start");
    state.phase = Phase::PeerShare;
    let reply_to = eff.me_ref().contramap(DriverMsg::Share);
    let _ps = register_peer_sharing_initiator_oneshot(
        &muxer,
        state.peer,
        state.conn_id,
        state.peer_share_amount,
        Duration::ZERO,
        reply_to,
        eff,
        DriverMsg::ChildDied,
    )
    .await;
    eff.set_timeout_at(3, state.protocol_timeout, DriverMsg::StepTimeout).await;
    state
}

async fn finish(mut state: Driver, eff: &Effects<DriverMsg>) -> Driver {
    if state.phase == Phase::Done {
        return state;
    }
    mark(&mut state, "session_done");
    state.phase = Phase::Done;
    // Deliver the report, then stay alive. Terminating the root stage signals graph-wide
    // abort and can kill the output stage before it forwards the report to the CLI.
    eff.send(&state.report_to, state.partial.clone()).await;
    state
}

fn format_refuse(reason: &RefuseReason) -> String {
    format!("{reason:?}")
}

#[cfg(test)]
mod tests {
    use super::{
        HandshakeInfo, PeerCheck, PsCheckExtras, PsCheckFailedAt, SessionPartial, civil_from_days, classify_failure,
    };

    #[test]
    fn unix_epoch_day_zero() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn known_date() {
        // 2000-01-01 = 10957 days since Unix epoch.
        assert_eq!(civil_from_days(10957), (2000, 1, 1));
    }

    #[test]
    fn classify_failure_maps_known_stages() {
        assert_eq!(classify_failure("TCP connect for session: timed out"), PsCheckFailedAt::Tcp);
        assert_eq!(classify_failure("handshake timed out"), PsCheckFailedAt::Handshake);
        assert_eq!(classify_failure("keep-alive ping timed out"), PsCheckFailedAt::Ping);
        assert_eq!(classify_failure("tip timed out"), PsCheckFailedAt::Tip);
        assert_eq!(classify_failure("peershare timed out"), PsCheckFailedAt::PeerShare);
    }

    #[test]
    fn peer_check_handshake_only_ok() {
        let partial = SessionPartial {
            handshake: Some(HandshakeInfo { version: Some(14), ..HandshakeInfo::default() }),
            ..SessionPartial::default()
        };
        let check = PeerCheck::from_session("1.2.3.4:3001", partial, PsCheckExtras::default());
        assert!(check.ok);
        assert_eq!(check.negotiated_version, Some(14));
        assert!(check.failed_at.is_none());
    }

}
