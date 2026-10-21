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

//! One-shot N2N probe against one or more live peers.
//!
//! Prefer `amaru peer probe <TARGETS> [OPTIONS]` so optional-value flags like `--ping` are not
//! fed a target address. Values for those flags use `=` (e.g. `--ping=5`).
//!
//! Business output goes to stdout (EDR-0019); silence Clippy print lints for this CLI.

#![expect(clippy::print_stdout)]
#![expect(clippy::print_literal)]
#![expect(clippy::wildcard_enum_match_arm)]

mod session;

use std::{
    io::{self, IsTerminal},
    str::FromStr,
    time::{Duration, Instant},
};

use amaru::{
    lifecycle::{Runnable, RuntimeKind},
    observability::Color,
};
use amaru_kernel::{NetworkName, Peer, PeerCandidate, utils::duration};
use amaru_network::resolve::{init_resolver, resolve_all_peers};
use amaru_protocols::protocol_messages::version_number::VersionNumber;
use anyhow::{Context, bail};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(override_usage = "amaru peer probe <TARGETS> [OPTIONS]")]
pub struct Args {
    /// Peer target(s), comma-separated.
    ///
    /// Each entry is `IP:PORT`, `HOSTNAME:PORT` (A/AAAA lookup), or a bare DNS name for a
    /// CIP-0155 Cardano SRV lookup (`_cardano._tcp.<name>`). SRV names expand to every usable
    /// record (RFC 2782 priority order); each resolved peer is probed in turn.
    #[arg(value_name = amaru::value_names::PEER_TARGETS)]
    targets: String,

    /// Run ping, handshake, tip, and peer-share.
    #[arg(long, help_heading = "Probe types", display_order = 1)]
    all: bool,

    /// Measure keep-alive mini-protocol RTT after TCP connect + N2N handshake.
    ///
    /// Optional value via `=`: `COUNT` (1..=86400, one ping per second) or `COUNT:INTERVAL`
    /// (e.g. `--ping=20:5s` = 20 pings, 5s apart). Bare `--ping` runs once. Use `=` so the
    /// next token is not taken as a ping count (write targets before options).
    #[arg(
        long,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "1",
        value_name = "COUNT|COUNT:INTERVAL",
        help_heading = "Probe types",
        display_order = 2,
    )]
    ping: Option<PingSpec>,

    /// Query remote offered N2N versions (`MsgQueryReply`) and print them.
    #[arg(long, help_heading = "Probe types", display_order = 3)]
    handshake: bool,

    /// Query tip via chain-sync FindIntersect(Origin).
    #[arg(long, help_heading = "Probe types", display_order = 4)]
    tip: bool,

    /// Request up to 10 peer-share addresses.
    #[arg(long = "peer-share", help_heading = "Probe types", display_order = 5)]
    peer_share: bool,

    /// After peer-share, check each returned peer on one TCP session (connect + N2N handshake).
    ///
    /// Bare `--peer-check` stops after a successful Accept. Optional semicolon-separated extras
    /// via `=`: `ping`, `tip`, `peer-share` (e.g. `--peer-check=tip;ping`). Peer-share extras
    /// report returned addresses but those addresses are not peer-checked again (one level only).
    /// Uses the same timeouts, `--requested-version`, `--time-track`, and `--retry` as the
    /// primary probe.
    #[arg(
        long = "peer-check",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "",
        value_name = "TESTS",
        help_heading = "Probe types",
        display_order = 6,
    )]
    peer_check: Option<PeerCheckSpec>,

    /// Cardano network (selects handshake network magic).
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value = "mainnet",
        help_heading = "Probe settings",
        display_order = 10,
    )]
    network: NetworkName,

    /// Highest N2N version to offer in the handshake (default: current Amaru N2N version).
    ///
    /// Proposes every Amaru-supported version from 11 through this value. The remote may
    /// Accept any offered version at or below this ceiling; a lower negotiated version is
    /// success, not a failure. Must be a version Amaru implements (11..=15 on this branch).
    /// Applies to the primary probe and to each `--peer-check` session.
    #[arg(
        long = "requested-version",
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::PROBE_REQUESTED_VERSION,
        default_value_t = VersionNumber::CURRENT.as_u64(),
        help_heading = "Probe settings",
        display_order = 11,
    )]
    requested_version: u64,

    /// Extra attempts after a failed connect, step timeout, or wall-clock `--timeout`.
    ///
    /// Optional value via `=`: `COUNT` (1..=32 retries, 500 ms apart) or `COUNT:INTERVAL`
    /// (e.g. `--retry=2:500ms`). Bare `--retry` retries once after 500 ms. Each attempt gets a
    /// fresh `--timeout` / step-timeout budget. Does not multiply `--ping` samples.
    #[arg(
        long,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "1:500ms",
        value_name = "COUNT|COUNT:INTERVAL",
        env = amaru::env_vars::PROBE_RETRY,
        help_heading = "Probe settings",
        display_order = 12,
    )]
    retry: Option<RetrySpec>,

    /// Wall-clock budget for one attempt on one target, from TCP connect start until the session
    /// finishes. When `--ping` requests several samples, the gaps between them
    /// (`(COUNT - 1) × INTERVAL`) are added on top so multi-ping runs are not cut short. Each
    /// `--retry` attempt gets a fresh budget. Step timeouts still apply within this.
    #[arg(
        long = "timeout",
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PROBE_TIMEOUT,
        default_value = "5s",
        help_heading = "Probe settings",
        display_order = 13,
    )]
    timeout: Duration,

    /// TCP connect timeout.
    #[arg(
        long = "timeout-connect",
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PROBE_TIMEOUT_CONNECT,
        default_value = "5s",
        help_heading = "Probe settings",
        display_order = 14,
    )]
    connect_timeout: Duration,

    /// Handshake mini-protocol timeout.
    #[arg(
        long = "timeout-handshake",
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PROBE_TIMEOUT_HANDSHAKE,
        default_value = "10s",
        help_heading = "Probe settings",
        display_order = 15,
    )]
    handshake_timeout: Duration,

    /// Timeout for chainsync / peer-share / keep-alive replies.
    #[arg(
        long = "timeout-protocol",
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PROBE_TIMEOUT_PROTOCOL,
        default_value = "15s",
        help_heading = "Probe settings",
        display_order = 16,
    )]
    protocol_timeout: Duration,

    /// Record per-step wall timestamps, deltas, elapsed-from-start, and TCP bytes.
    #[arg(long = "time-track", help_heading = "Probe settings", display_order = 17)]
    time_track: bool,

    /// Emit machine-readable JSON instead of color human output.
    #[arg(long, help_heading = "Probe settings", display_order = 18)]
    json: bool,
}

/// How many keep-alive pings to run and the gap between them.
#[derive(Debug, Clone, Copy)]
struct PingSpec {
    count: u32,
    interval: Duration,
}

impl FromStr for PingSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (count, interval) = parse_count_interval(s, Duration::from_secs(1), "ping", 1, 86400)?;
        Ok(Self { count, interval })
    }
}

/// Optional post-handshake tests for `--peer-check` (semicolon-separated).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PeerCheckSpec {
    extras: session::PsCheckExtras,
}

impl PeerCheckSpec {
    fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.extras.ping {
            parts.push("ping");
        }
        if self.extras.tip {
            parts.push("tip");
        }
        if self.extras.peer_share {
            parts.push("peer-share");
        }
        if parts.is_empty() { "handshake".to_string() } else { parts.join(";") }
    }
}

impl FromStr for PeerCheckSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut extras = session::PsCheckExtras::default();
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Ok(Self { extras });
        }
        for raw in trimmed.split(';') {
            let token = raw.trim();
            if token.is_empty() {
                continue;
            }
            match token.to_ascii_lowercase().as_str() {
                "ping" => extras.ping = true,
                "tip" => extras.tip = true,
                "peer-share" => extras.peer_share = true,
                other => {
                    return Err(format!(
                        "unknown --peer-check test '{other}'; expected ping, tip, peer-share \
                         (semicolon-separated)"
                    ));
                }
            }
        }
        Ok(Self { extras })
    }
}

/// Extra attempts after the first failure, and the delay between attempts.
#[derive(Debug, Clone, Copy)]
struct RetrySpec {
    /// Number of retries after the initial attempt (total attempts = `count + 1`).
    count: u32,
    interval: Duration,
}

impl FromStr for RetrySpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (count, interval) = parse_count_interval(s, Duration::from_millis(500), "retry", 1, 32)?;
        Ok(Self { count, interval })
    }
}

fn parse_count_interval(
    s: &str,
    default_interval: Duration,
    kind: &str,
    min_count: u32,
    max_count: u32,
) -> Result<(u32, Duration), String> {
    let (count_str, interval) = if let Some((count_str, interval_str)) = s.split_once(':') {
        // Bare digits mean milliseconds (legacy); otherwise `duration::parse` (`5s`, `500ms`, …).
        let interval = if interval_str.chars().all(|c| c.is_ascii_digit()) {
            let interval_ms: u64 =
                interval_str.parse().map_err(|_| format!("invalid {kind} interval '{interval_str}'"))?;
            if interval_ms == 0 {
                return Err(format!("{kind} interval must be >= 1 ms"));
            }
            Duration::from_millis(interval_ms)
        } else {
            duration::parse(interval_str).map_err(|e| format!("invalid {kind} interval: {e}"))?
        };
        (count_str, interval)
    } else {
        (s, default_interval)
    };

    let count: u32 = count_str
        .parse()
        .map_err(|_| format!("invalid {kind} count '{count_str}' (expected integer {min_count}..={max_count})"))?;
    if !(min_count..=max_count).contains(&count) {
        return Err(format!("{kind} count must be between {min_count} and {max_count}"));
    }

    Ok((count, interval))
}

/// Run `op` once, then up to `retry.count` more times after `retry.interval` on `Err`.
async fn with_retries<T, E, F, Fut>(retry: Option<RetrySpec>, op: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    with_retries_if(retry, |_| false, op).await
}

/// Like [`with_retries`], but also retries when `Ok(value)` and `retry_ok(&value)` is true.
///
/// After retries are exhausted, returns the last `Ok(value)` that failed `retry_ok` (so callers
/// still see step-timeout details) or the last `Err`.
async fn with_retries_if<T, E, F, Fut, R>(retry: Option<RetrySpec>, mut retry_ok: R, mut op: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    R: FnMut(&T) -> bool,
{
    let (extra, delay) = match retry {
        Some(spec) => (spec.count, spec.interval),
        None => (0, Duration::ZERO),
    };
    let mut attempt = 0u32;
    loop {
        match op().await {
            Ok(value) if !retry_ok(&value) => return Ok(value),
            Ok(value) => {
                if attempt < extra {
                    attempt += 1;
                    tokio::time::sleep(delay).await;
                    continue;
                }
                // Retries exhausted: keep the last report so step-timeout details are visible.
                return Ok(value);
            }
            Err(_err) if attempt < extra => {
                attempt += 1;
                tokio::time::sleep(delay).await;
            }
            Err(err) => return Err(err),
        }
    }
}

fn session_partial_needs_retry(partial: &session::SessionPartial) -> bool {
    !partial.errors.is_empty()
}

#[derive(Debug, Clone, Copy)]
struct Actions {
    ping: Option<PingSpec>,
    handshake: bool,
    peer_share: bool,
    peer_check: Option<PeerCheckSpec>,
    tip: bool,
}

impl Actions {
    fn from_args(args: &Args) -> Self {
        if args.all {
            return Self {
                // Keep an explicit `--ping=…` / `--peer-check=…`; default one ping when absent.
                ping: args.ping.or(Some(PingSpec { count: 1, interval: Duration::from_secs(1) })),
                handshake: true,
                peer_share: true,
                peer_check: args.peer_check,
                tip: true,
            };
        }
        let mut actions = Self {
            ping: args.ping,
            handshake: args.handshake,
            peer_share: args.peer_share || args.peer_check.is_some(),
            peer_check: args.peer_check,
            tip: args.tip,
        };
        if actions.ping.is_none()
            && !(actions.handshake || actions.peer_share || actions.peer_check.is_some() || actions.tip)
        {
            // Default: full probe when no action flag is given.
            actions = Self {
                ping: Some(PingSpec { count: 1, interval: Duration::from_secs(1) }),
                handshake: true,
                peer_share: true,
                peer_check: None,
                tip: true,
            };
        }
        actions
    }

    fn needs_session(self) -> bool {
        self.ping.is_some() || self.peer_share || self.tip
    }
}

/// Wall-clock budget for one attempt: base `--timeout` plus intentional inter-ping waits.
fn session_timeout_for(base: Duration, ping: Option<PingSpec>) -> Duration {
    match ping {
        Some(PingSpec { count, interval }) if count > 1 => {
            base.saturating_add(interval.saturating_mul(count.saturating_sub(1)))
        }
        Some(_) | None => base,
    }
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

/// Split a comma-separated target list into [`PeerCandidate`]s.
fn parse_targets(raw: &str) -> anyhow::Result<Vec<PeerCandidate>> {
    let targets: Vec<PeerCandidate> = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<PeerCandidate>()
                .with_context(|| format!("invalid peer target '{part}' (expected IP:PORT, HOST:PORT, or CIP-0155 DNS name)"))
        })
        .collect::<anyhow::Result<_>>()?;
    if targets.is_empty() {
        bail!("at least one peer target is required");
    }
    Ok(targets)
}

async fn run(args: Args) -> anyhow::Result<()> {
    let candidates = parse_targets(&args.targets)?;
    // Needed for CIP-0155 SRV lookups; host A/AAAA uses tokio lookup and is unaffected.
    init_resolver().context("failed to initialise DNS resolver for peer probe")?;

    let actions = Actions::from_args(&args);
    let max_n2n_version = resolve_requested_version(args.requested_version)?;
    let color = color_enabled();
    // Stream long-running human steps as they complete; JSON stays one final document.
    let stream = !args.json;

    let mut reports = Vec::with_capacity(candidates.len());
    for candidate in &candidates {
        let peers = resolve_all_peers(candidate).await;
        if peers.is_empty() {
            reports.push(probe_resolve_failed(candidate, &args, stream, color));
            continue;
        }
        for peer in peers {
            let report =
                probe_one(candidate, peer, &args, actions, max_n2n_version, stream, color).await;
            reports.push(report);
        }
    }

    if args.json {
        emit_json(&reports)?;
    }

    if reports.iter().any(|r| !r.errors.is_empty()) {
        bail!("probe completed with errors");
    }
    Ok(())
}

fn probe_resolve_failed(
    candidate: &PeerCandidate,
    args: &Args,
    stream: bool,
    color: bool,
) -> session::ProbeReport {
    let out = HumanOut::new(color);
    let address = candidate.to_string();
    let kind = match candidate {
        PeerCandidate::Socket(_) => "socket",
        PeerCandidate::Host { .. } => "host",
        PeerCandidate::Srv { .. } => "CIP-0155 SRV",
    };
    let report = session::ProbeReport {
        address,
        resolved: None,
        network: args.network.to_string(),
        errors: vec![format!("failed to resolve {kind} target '{candidate}'")],
        ..session::ProbeReport::default()
    };
    if stream {
        out.probe_header(&report);
        out.errors(&report.errors);
    }
    report
}

async fn probe_one(
    candidate: &PeerCandidate,
    peer: Peer,
    args: &Args,
    actions: Actions,
    max_n2n_version: VersionNumber,
    stream: bool,
    color: bool,
) -> session::ProbeReport {
    let out = HumanOut::new(color);
    let address = candidate.to_string();

    let mut report = session::ProbeReport {
        address: address.clone(),
        resolved: candidate.needs_resolution().then(|| peer.to_string()),
        network: args.network.to_string(),
        ping_rtts_ms: Vec::new(),
        ping_interval_ms: None,
        handshake: None,
        peers: None,
        tip: None,
        peer_checks: Vec::new(),
        timetrack: Vec::new(),
        errors: Vec::new(),
    };

    // Multi-ping gaps extend only the primary session budget, not query / peer-check attempts.
    let session_timeout = session_timeout_for(args.timeout, actions.ping);
    let step_timeout = args.timeout;
    let connect_timeout = args.connect_timeout;
    let handshake_timeout = args.handshake_timeout;
    let protocol_timeout = args.protocol_timeout;

    // Shared timeline clock across query + session when `--time-track` is set.
    let mut track_clock: Option<(Instant, Instant)> = None;
    if args.time_track {
        let started = Instant::now();
        track_clock = Some((started, started));
    }

    if stream {
        out.probe_header(&report);
    }

    // --handshake: dedicated query connection listing every remote-offered N2N version.
    if actions.handshake {
        match with_retries(args.retry, || {
            session::run_version_query(
                peer,
                args.network,
                step_timeout,
                connect_timeout,
                handshake_timeout,
                max_n2n_version,
                args.time_track,
                track_clock,
            )
        })
        .await
        {
            Ok((offered, steps)) => {
                report.handshake =
                    Some(session::HandshakeInfo { offered_versions: offered, ..session::HandshakeInfo::default() });
                if args.time_track {
                    report.timetrack.extend(steps);
                    if let Some((_, last)) = track_clock.as_mut() {
                        *last = Instant::now();
                    }
                }
            }
            Err(err) => {
                // Soft when a session Accept will follow: the query connection is separate and
                // flaky under load; negotiated version still comes from the Accept handshake.
                if actions.needs_session() {
                    if stream {
                        out.warn_line(format!("handshake query: {err:#}"));
                    }
                } else {
                    report.errors.push(format!("handshake query: {err:#}"));
                }
            }
        }
        if stream && !actions.needs_session() {
            if let Some(hs) = &report.handshake {
                out.handshake(hs);
            }
            out.errors(&report.errors);
        }
    }

    if let Some(spec) = actions.ping {
        report.ping_interval_ms = Some(spec.interval.as_millis() as u64);
    }

    if actions.needs_session() {
        let stream_ping = stream && actions.ping.is_some();
        let mut ping_streamed = false;
        match with_retries_if(
            args.retry,
            |(partial, _)| session_partial_needs_retry(partial),
            || {
                run_session_streaming_ping(
                    session::SessionRequest {
                        peer,
                        network: args.network,
                        session_timeout,
                        connect_timeout,
                        handshake_timeout,
                        protocol_timeout,
                        want_peershare: actions.peer_share,
                        want_tip: actions.tip,
                        want_ping: actions.ping.map(|s| (s.count, s.interval)),
                        ping_progress: None,
                        want_timetrack: args.time_track,
                        timetrack_continue: track_clock,
                        max_n2n_version,
                        peer_share_amount: 10,
                    },
                    stream_ping,
                    &out,
                    report.ping_interval_ms,
                )
            },
        )
        .await
        {
            Ok((partial, streamed)) => {
                ping_streamed = streamed;
                report.ping_rtts_ms = partial.ping_rtts_ms;
                // Merge Accept negotiated fields onto any prior query offered_versions.
                match (report.handshake.take(), partial.handshake) {
                    (Some(mut hs), Some(accepted)) => {
                        hs.version = accepted.version;
                        hs.network_magic = accepted.network_magic;
                        hs.initiator_only = accepted.initiator_only;
                        hs.peer_sharing = accepted.peer_sharing;
                        hs.query = accepted.query;
                        if hs.offered_versions.is_empty() {
                            hs.offered_versions = accepted.offered_versions;
                        }
                        report.handshake = Some(hs);
                    }
                    (None, accepted) => report.handshake = accepted,
                    (prior, None) => report.handshake = prior,
                }
                report.peers = partial.peers;
                report.tip = partial.tip;
                report.errors.extend(partial.errors);
                if args.time_track {
                    report.timetrack.extend(partial.timetrack);
                }
            }
            Err(err) => report.errors.push(format!("session: {err:#}")),
        }
        if stream {
            // Samples already printed live; finish the ping section before Accept/tip/share.
            if ping_streamed {
                out.ping_summary(&report);
            }
            if let Some(hs) = &report.handshake {
                out.handshake(hs);
            }
            if !ping_streamed && !report.ping_rtts_ms.is_empty() {
                out.ping(&report);
            }
            out.session_sections(&report, /*include_peer_check*/ false, /*skip_handshake*/ true);
        }
    } else if stream && actions.handshake {
        // handshake-only already streamed above
    }

    if stream && args.time_track && !report.timetrack.is_empty() {
        out.timetrack(&report.timetrack);
    }

    if let Some(check_spec) = actions.peer_check {
        let peers = report.peers.clone().unwrap_or_default();
        let mut peer_check_header = stream && !peers.is_empty();
        let check_params = PeerCheckParams {
            network: args.network,
            session_timeout: step_timeout,
            connect_timeout,
            handshake_timeout,
            protocol_timeout,
            max_n2n_version,
            extras: check_spec.extras,
            time_track: args.time_track,
        };
        for addr in peers {
            let check = match with_retries_if(args.retry, |c: &session::PeerCheck| !c.ok, || {
                run_peer_check_one(&addr, check_params)
            })
            .await
            {
                Ok(check) => check,
                Err(err) => session::PeerCheck::hard_failure(&addr, format!("{err:#}")),
            };
            if stream {
                if peer_check_header {
                    out.peer_check_header();
                    peer_check_header = false;
                }
                out.peer_check_nested(&check);
            }
            report.peer_checks.push(check);
        }
    }

    report
}

/// Run a probe session, optionally streaming each keep-alive RTT as it arrives.
///
/// Returns `(partial, ping_samples_were_streamed)`.
async fn run_session_streaming_ping(
    mut req: session::SessionRequest,
    stream_ping: bool,
    out: &HumanOut,
    ping_interval_ms: Option<u64>,
) -> anyhow::Result<(session::SessionPartial, bool)> {
    if !stream_ping {
        let partial = session::run_session(req).await?;
        return Ok((partial, false));
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    req.ping_progress = Some(tx);
    let session = session::run_session(req);
    tokio::pin!(session);

    let mut seq = 0u32;
    let mut header = false;
    let print_sample = |seq: u32, ms: f64, header: &mut bool| {
        if !*header {
            out.ping_header(ping_interval_ms);
            *header = true;
        }
        out.ping_sample(seq, ms);
    };

    let partial = loop {
        tokio::select! {
            biased;
            sample = rx.recv() => {
                if let Some(ms) = sample {
                    seq += 1;
                    print_sample(seq, ms, &mut header);
                }
            }
            result = &mut session => {
                while let Ok(ms) = rx.try_recv() {
                    seq += 1;
                    print_sample(seq, ms, &mut header);
                }
                break result?;
            }
        }
    };
    Ok((partial, header))
}

/// Shared probe settings applied to each `--peer-check` session.
#[derive(Debug, Clone, Copy)]
struct PeerCheckParams {
    network: NetworkName,
    session_timeout: Duration,
    connect_timeout: Duration,
    handshake_timeout: Duration,
    protocol_timeout: Duration,
    max_n2n_version: VersionNumber,
    extras: session::PsCheckExtras,
    time_track: bool,
}

/// One TCP connect + handshake (+ optional extras). Hard connect/session errors are `Err`
/// so `--retry` can re-attempt; soft handshake/protocol failures are `Ok` with `ok: false`.
async fn run_peer_check_one(addr: &str, params: PeerCheckParams) -> anyhow::Result<session::PeerCheck> {
    let Ok(peer) = addr.parse::<Peer>() else {
        // Not retryable: address came from peer-share encoding.
        return Ok(session::PeerCheck::hard_failure(addr, "invalid peer address from share"));
    };

    // Peer-share extras are reported on this check only; callers never enqueue those peers
    // for another peer-check (one discovery level).
    let partial = session::run_session(session::SessionRequest {
        peer,
        network: params.network,
        session_timeout: params.session_timeout,
        connect_timeout: params.connect_timeout,
        handshake_timeout: params.handshake_timeout,
        protocol_timeout: params.protocol_timeout,
        want_peershare: params.extras.peer_share,
        want_tip: params.extras.tip,
        want_ping: params.extras.ping.then_some((1, Duration::from_secs(1))),
        ping_progress: None,
        want_timetrack: params.time_track,
        timetrack_continue: None,
        max_n2n_version: params.max_n2n_version,
        peer_share_amount: 10,
    })
    .await?;

    Ok(session::PeerCheck::from_session(addr, partial, params.extras))
}

/// One object for a single target; a JSON array when several targets were probed.
fn emit_json(reports: &[session::ProbeReport]) -> anyhow::Result<()> {
    if reports.len() == 1 {
        println!("{}", serde_json::to_string_pretty(&reports[0])?);
    } else {
        println!("{}", serde_json::to_string_pretty(reports)?);
    }
    Ok(())
}

/// ANSI (or empty) color codes for human probe output.
struct Palette {
    ok: &'static str,
    warn: &'static str,
    err: &'static str,
    cyan: &'static str,
    bold: &'static str,
    reset: &'static str,
}

impl Palette {
    fn new(color: bool) -> Self {
        if color {
            Self {
                ok: "\x1b[32m",
                warn: "\x1b[33m",
                err: "\x1b[31m",
                cyan: "\x1b[36m",
                bold: "\x1b[1m",
                reset: "\x1b[0m",
            }
        } else {
            Self { ok: "", warn: "", err: "", cyan: "", bold: "", reset: "" }
        }
    }

    fn status(&self, ok: bool) -> &'static str {
        if ok { self.ok } else { self.warn }
    }
}

/// Shared helpers for human-readable probe sections (streaming and buffered).
struct HumanOut {
    p: Palette,
}

impl HumanOut {
    fn new(color: bool) -> Self {
        Self { p: Palette::new(color) }
    }

    fn flush(&self) {
        let _ = io::Write::flush(&mut io::stdout());
    }

    /// Aligned `key: value` line under a section.
    fn field(&self, indent: &str, key: &str, value: impl std::fmt::Display) {
        let label = format!("{key}:");
        println!("{indent}{label:<18} {value}");
    }

    fn section_ok(&self, name: &str) {
        println!("  {}{name}{}", self.p.ok, self.p.reset);
    }

    fn section_ok_suffix(&self, name: &str, suffix: impl std::fmt::Display) {
        println!("  {}{name}{}  {suffix}", self.p.ok, self.p.reset);
    }

    fn section_warn_suffix(&self, name: &str, suffix: impl std::fmt::Display) {
        println!("  {}{name}{}  {suffix}", self.p.warn, self.p.reset);
    }

    fn probe_header(&self, report: &session::ProbeReport) {
        match &report.resolved {
            Some(resolved) => println!(
                "{}probe{} {}{}{} -> {}{}{} ({})",
                self.p.bold,
                self.p.reset,
                self.p.cyan,
                report.address,
                self.p.reset,
                self.p.cyan,
                resolved,
                self.p.reset,
                report.network
            ),
            None => println!(
                "{}probe{} {}{}{} ({})",
                self.p.bold, self.p.reset, self.p.cyan, report.address, self.p.reset, report.network
            ),
        }
        self.flush();
    }

    fn handshake(&self, hs: &session::HandshakeInfo) {
        self.section_ok("handshake");
        if !hs.offered_versions.is_empty() {
            let list = hs.offered_versions.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");
            self.field("    ", "offered_versions", format!("[{list}]"));
        }
        if let Some(v) = hs.version {
            self.field("    ", "version", v);
        }
        if let Some(m) = hs.network_magic {
            self.field("    ", "network_magic", m);
        }
        if let Some(ps) = hs.peer_sharing {
            self.field("    ", "peer_sharing", ps);
        }
        if let Some(io) = hs.initiator_only {
            self.field("    ", "initiator_only", io);
        }
        if let Some(q) = hs.query {
            self.field("    ", "query", q);
        }
        self.flush();
    }

    /// Tip / peershare (and optionally already-collected peer-check rows).
    fn session_sections(&self, report: &session::ProbeReport, include_peer_check: bool, skip_handshake: bool) {
        if !skip_handshake && let Some(hs) = &report.handshake {
            self.handshake(hs);
        }
        if let Some(tip) = &report.tip {
            self.section_ok("tip");
            self.field("    ", "slot", tip.slot);
            self.field("    ", "block_height", tip.block_height);
            self.field("    ", "hash", &tip.hash);
        }
        if let Some(peers) = &report.peers {
            if peers.is_empty() {
                self.section_warn_suffix("peer-share", "skipped (not negotiated)");
            } else {
                self.section_ok_suffix("peer-share", format!("{} peer(s)", peers.len()));
                for p in peers {
                    if include_peer_check && let Some(check) = report.peer_checks.iter().find(|c| c.address == *p) {
                        self.peer_check_nested(check);
                        continue;
                    }
                    println!("    {p}");
                }
            }
        }
        if include_peer_check {
            for check in &report.peer_checks {
                if report.peers.as_ref().is_some_and(|ps| ps.iter().any(|p| p == &check.address)) {
                    continue;
                }
                self.peer_check_orphan(check);
            }
        }
        self.errors(&report.errors);
        self.flush();
    }

    /// Buffered (JSON / non-stream) ping section: samples then optional summary.
    fn ping(&self, report: &session::ProbeReport) {
        let samples = &report.ping_rtts_ms;
        self.ping_header(report.ping_interval_ms);
        if samples.len() == 1 {
            self.field("    ", "rtt", format!("{} ms", format_rtt_ms(samples[0])));
            self.flush();
            return;
        }
        for (i, ms) in samples.iter().enumerate() {
            self.ping_sample(i as u32 + 1, *ms);
        }
        self.ping_summary(report);
    }

    fn ping_header(&self, interval_ms: Option<u64>) {
        self.section_ok("ping");
        if let Some(interval) = interval_ms {
            self.field("    ", "interval", format!("{interval} ms"));
        }
        self.flush();
    }

    fn ping_sample(&self, seq: u32, ms: f64) {
        self.field("    ", &seq.to_string(), format!("{} ms", format_rtt_ms(ms)));
        self.flush();
    }

    /// min/avg/max after streamed (or multi-sample) pings. No-op for a single sample.
    fn ping_summary(&self, report: &session::ProbeReport) {
        let samples = &report.ping_rtts_ms;
        if samples.len() < 2 {
            return;
        }
        let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let max = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let avg = samples.iter().sum::<f64>() / samples.len() as f64;
        self.field("    ", "count", samples.len());
        self.field("    ", "min", format!("{} ms", format_rtt_ms(min)));
        self.field("    ", "avg", format!("{} ms", format_rtt_ms(avg)));
        self.field("    ", "max", format!("{} ms", format_rtt_ms(max)));
        self.flush();
    }

    fn timetrack(&self, steps: &[session::TimeTrackStep]) {
        self.section_ok("time-track");
        println!("    {:<22} {:>10} {:>10} {:>10} {:>10}  {}", "step", "delta_ms", "elapsed_ms", "sent", "recv", "at");
        for s in steps {
            println!(
                "    {:<22} {:>10} {:>10} {:>10} {:>10}  {}",
                s.step, s.delta_ms, s.elapsed_ms, s.bytes_sent, s.bytes_recv, s.at
            );
        }
        if let Some(last) = steps.last() {
            println!(
                "    total_elapsed_ms:  {}  bytes_sent: {}  bytes_recv: {}",
                last.elapsed_ms, last.bytes_sent, last.bytes_recv
            );
        }
        self.flush();
    }

    fn errors(&self, errors: &[String]) {
        if errors.is_empty() {
            return;
        }
        for e in errors {
            println!("  {}error{}   {e}", self.p.err, self.p.reset);
        }
        self.flush();
    }

    fn warn_line(&self, message: impl std::fmt::Display) {
        println!("  {}warn{}    {message}", self.p.err, self.p.reset);
        self.flush();
    }

    fn peer_check_header(&self) {
        self.section_ok("peer-check");
        self.flush();
    }

    /// Nested under a peershare address, or a live streaming progress line.
    fn peer_check_nested(&self, check: &session::PeerCheck) {
        let tag = self.p.status(check.ok);
        println!("    {}  {tag}{}{}", check.address, format_peer_check_summary(check), self.p.reset);
        self.peer_check_details(check, "      ");
        self.flush();
    }

    fn peer_check_orphan(&self, check: &session::PeerCheck) {
        let tag = self.p.status(check.ok);
        println!("  {tag}peer-check{}  {}  {}", self.p.reset, check.address, format_peer_check_summary(check));
        self.peer_check_details(check, "           ");
    }

    fn peer_check_details(&self, check: &session::PeerCheck, indent: &str) {
        if !check.ping_rtts_ms.is_empty() {
            let samples =
                check.ping_rtts_ms.iter().map(|ms| format!("{} ms", format_rtt_ms(*ms))).collect::<Vec<_>>().join(", ");
            self.field(indent, "ping", samples);
        }
        if let Some(tip) = &check.tip {
            self.field(indent, "tip slot", tip.slot);
            self.field(indent, "tip height", tip.block_height);
            self.field(indent, "tip hash", &tip.hash);
        }
        if let Some(peers) = &check.peers {
            self.field(indent, "peer-share", format!("{} peer(s) (not recursively checked)", peers.len()));
            for p in peers {
                println!("{indent}  {p}");
            }
        }
        if !check.timetrack.is_empty() {
            println!("{indent}time-track:");
            for s in &check.timetrack {
                println!("{indent}  {:<22} {:>10} {:>10}", s.step, s.delta_ms, s.elapsed_ms);
            }
        }
    }

}

fn format_peer_check_summary(check: &session::PeerCheck) -> String {
    let version = check.negotiated_version.map(|v| format!(" v{v}")).unwrap_or_default();
    if check.ok {
        format!("ok handshake{version}")
    } else {
        let stage = check.failed_at.map(|s| s.as_str()).unwrap_or("unknown");
        format!("fail {stage}: {}", check.detail)
    }
}

fn format_rtt_ms(ms: f64) -> String {
    if ms >= 10.0 {
        format!("{ms:.1}")
    } else if ms >= 1.0 {
        format!("{ms:.2}")
    } else {
        format!("{ms:.3}")
    }
}

fn color_enabled() -> bool {
    let mode =
        std::env::var("AMARU_COLOR").ok().as_deref().and_then(|s| Color::from_str(s).ok()).unwrap_or(Color::Auto);
    match mode {
        Color::Never => false,
        Color::Always => true,
        Color::Auto => {
            if std::env::var("NO_COLOR").iter().any(|s| !s.is_empty()) {
                false
            } else {
                io::stdout().is_terminal()
            }
        }
    }
}

fn resolve_requested_version(raw: u64) -> anyhow::Result<VersionNumber> {
    let version = VersionNumber::new(raw);
    if version.is_supported() {
        return Ok(version);
    }
    let supported = VersionNumber::SUPPORTED.iter().map(|v| v.as_u64().to_string()).collect::<Vec<_>>().join(", ");
    bail!(
        "unsupported --requested-version {raw}; Amaru speaks N2N versions [{supported}] (default {})",
        VersionNumber::CURRENT.as_u64()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_targets_splits_comma_separated_socket_host_and_srv() {
        let targets = parse_targets("10.0.0.1:3001, relay.example:3002, pool.example").unwrap();
        assert_eq!(targets.len(), 3);
        assert!(matches!(targets[0], PeerCandidate::Socket(_)));
        assert!(matches!(targets[1], PeerCandidate::Host { .. }));
        assert!(matches!(targets[2], PeerCandidate::Srv { .. }));
    }

    #[test]
    fn parse_targets_rejects_empty_list() {
        assert!(parse_targets(" , , ").is_err());
    }

    #[test]
    fn parse_targets_rejects_literal_ip_without_port() {
        let err = parse_targets("10.0.0.1").unwrap_err().to_string();
        assert!(err.contains("10.0.0.1"));
    }

    #[test]
    fn clap_keeps_target_when_bare_ping_precedes_address() {
        // `--ping` requires `=` for a value, so the following token is the target.
        let args = Args::try_parse_from(["probe", "--ping", "1.2.3.4:5678"]).unwrap();
        assert_eq!(args.targets, "1.2.3.4:5678");
        assert_eq!(args.ping.map(|p| p.count), Some(1));
    }

    #[test]
    fn clap_accepts_targets_before_options() {
        let args = Args::try_parse_from(["probe", "relay.example:3001,pool.example", "--ping=2"]).unwrap();
        assert_eq!(args.targets, "relay.example:3001,pool.example");
        assert_eq!(args.ping.map(|p| p.count), Some(2));
    }

    #[test]
    fn peer_check_spec_bare_is_handshake_only() {
        let spec: PeerCheckSpec = "".parse().unwrap();
        assert_eq!(spec.extras, session::PsCheckExtras::default());
        assert_eq!(spec.display(), "handshake");
    }

    #[test]
    fn peer_check_spec_parses_semicolon_extras() {
        let spec: PeerCheckSpec = "ping;tip;peer-share".parse().unwrap();
        assert!(spec.extras.ping);
        assert!(spec.extras.tip);
        assert!(spec.extras.peer_share);
        assert_eq!(spec.display(), "ping;tip;peer-share");
    }

    #[test]
    fn peer_check_spec_rejects_unknown_token() {
        let err = "tip;foo".parse::<PeerCheckSpec>().unwrap_err();
        assert!(err.contains("unknown"));
    }

    #[test]
    fn retry_spec_parses_count_and_interval() {
        let spec: RetrySpec = "2:500".parse().unwrap();
        assert_eq!(spec.count, 2);
        assert_eq!(spec.interval, Duration::from_millis(500));
        let with_unit: RetrySpec = "2:500ms".parse().unwrap();
        assert_eq!(with_unit.interval, Duration::from_millis(500));
    }

    #[test]
    fn retry_spec_bare_count_defaults_to_500ms() {
        let spec: RetrySpec = "3".parse().unwrap();
        assert_eq!(spec.count, 3);
        assert_eq!(spec.interval, Duration::from_millis(500));
    }

    #[test]
    fn ping_spec_bare_count_defaults_to_1s() {
        let spec: PingSpec = "5".parse().unwrap();
        assert_eq!(spec.count, 5);
        assert_eq!(spec.interval, Duration::from_secs(1));
    }

    #[tokio::test]
    async fn with_retries_succeeds_on_later_attempt() {
        let mut tries = 0u32;
        let result = with_retries(Some(RetrySpec { count: 2, interval: Duration::from_millis(1) }), || {
            tries += 1;
            async move { if tries < 3 { Err("transient") } else { Ok(42) } }
        })
        .await;
        assert_eq!(result, Ok(42));
        assert_eq!(tries, 3);
    }

    #[tokio::test]
    async fn with_retries_none_is_single_attempt() {
        let mut tries = 0u32;
        let result: Result<(), &str> = with_retries(None, || {
            tries += 1;
            async { Err("fail") }
        })
        .await;
        assert_eq!(result, Err("fail"));
        assert_eq!(tries, 1);
    }

    #[tokio::test]
    async fn with_retries_if_retries_ok_failures_then_returns_last() {
        let mut tries = 0u32;
        let result = with_retries_if(
            Some(RetrySpec { count: 2, interval: Duration::from_millis(1) }),
            |n: &u32| *n < 10,
            || {
                tries += 1;
                async move { Ok::<u32, &str>(tries) }
            },
        )
        .await;
        assert_eq!(result, Ok(3));
        assert_eq!(tries, 3);
    }

    #[test]
    fn session_timeout_adds_inter_ping_gaps() {
        let base = Duration::from_secs(5);
        assert_eq!(session_timeout_for(base, None), base);
        assert_eq!(
            session_timeout_for(base, Some(PingSpec { count: 1, interval: Duration::from_secs(5) })),
            base
        );
        assert_eq!(
            session_timeout_for(base, Some(PingSpec { count: 20, interval: Duration::from_secs(5) })),
            Duration::from_secs(5 + 19 * 5)
        );
    }

    #[test]
    fn all_preserves_explicit_ping_and_peer_check() {
        let args = Args::try_parse_from([
            "probe",
            "1.2.3.4:3001",
            "--all",
            "--ping=3:2s",
            "--peer-check=tip",
        ])
        .unwrap();
        let actions = Actions::from_args(&args);
        assert_eq!(actions.ping.map(|p| (p.count, p.interval)), Some((3, Duration::from_secs(2))));
        assert!(actions.peer_check.is_some());
        assert!(actions.peer_check.unwrap().extras.tip);
    }

    #[tokio::test]
    async fn with_retries_if_stops_when_ok_succeeds() {
        let mut tries = 0u32;
        let result = with_retries_if(
            Some(RetrySpec { count: 5, interval: Duration::from_millis(1) }),
            |n: &u32| *n < 2,
            || {
                tries += 1;
                async move { Ok::<u32, &str>(tries) }
            },
        )
        .await;
        assert_eq!(result, Ok(2));
        assert_eq!(tries, 2);
    }
}
