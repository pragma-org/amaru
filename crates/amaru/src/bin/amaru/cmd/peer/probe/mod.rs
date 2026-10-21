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

//! One-shot N2N probe against a live peer.
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
use amaru_kernel::{NetworkName, Peer, utils::duration};
use amaru_observability::info;
use amaru_protocols::protocol_messages::version_number::VersionNumber;
use anyhow::{Context, bail};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// Remote node address (`IP:PORT`).
    #[arg(value_name = amaru::value_names::HOST_PORT)]
    address: String,

    /// Cardano network (selects handshake network magic).
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value = "mainnet",
        display_order = 0,
    )]
    network: NetworkName,

    /// TCP connect timeout.
    #[arg(
        long = "timeout-connect",
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PROBE_TIMEOUT_CONNECT,
        default_value = "5s",
        display_order = 0,
    )]
    connect_timeout: Duration,

    /// Handshake mini-protocol timeout.
    #[arg(
        long = "timeout-handshake",
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PROBE_TIMEOUT_HANDSHAKE,
        default_value = "10s",
        display_order = 0,
    )]
    handshake_timeout: Duration,

    /// Timeout for chainsync / peer-share / keep-alive replies.
    #[arg(
        long = "timeout-protocol",
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PROBE_TIMEOUT_PROTOCOL,
        default_value = "15s",
        display_order = 0,
    )]
    protocol_timeout: Duration,

    /// Measure keep-alive mini-protocol RTT after TCP connect + N2N handshake.
    ///
    /// Optional value: `COUNT` (1..=86400, one ping per second) or `COUNT:INTERVAL`
    /// (e.g. `20:5s` = 20 pings, 5s apart). Bare `--ping` runs once.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "1",
        value_name = "COUNT|COUNT:INTERVAL",
        display_order = 0,
    )]
    ping: Option<PingSpec>,

    /// Query remote offered N2N versions (`MsgQueryReply`) and print them.
    #[arg(long, display_order = 0)]
    handshake: bool,

    /// Request up to 10 peer-share addresses.
    #[arg(long = "peer-share", display_order = 0)]
    peer_share: bool,

    /// After peer-share, check each returned peer on one TCP session (connect + N2N handshake).
    ///
    /// Bare `--ps-check` stops after a successful Accept. Optional semicolon-separated extras
    /// run on the same connection after handshake: `ping`, `tip`, `peer-share`
    /// (e.g. `--ps-check tip;ping`). Peer-share extras report returned addresses
    /// but those addresses are not ps-checked again (one level only). Uses the same timeouts,
    /// `--requested-version`, `--time-track`, and `--retry` as the primary probe.
    #[arg(
        long = "ps-check",
        num_args = 0..=1,
        default_missing_value = "",
        value_name = "TESTS",
        display_order = 0,
    )]
    ps_check: Option<PsCheckSpec>,

    /// Query tip via chain-sync FindIntersect(Origin).
    #[arg(long, display_order = 0)]
    tip: bool,

    /// Run ping, handshake, tip, and peer-share (then bare ps-check if peers are returned).
    #[arg(long, display_order = 0)]
    all: bool,

    /// Record per-step wall timestamps, deltas, elapsed-from-start, and TCP bytes.
    #[arg(long = "time-track", display_order = 0)]
    time_track: bool,

    /// Extra attempts after a failed handshake-query or session step.
    ///
    /// Optional value: `COUNT` (1..=32 retries, 500 ms apart) or `COUNT:INTERVAL`
    /// (e.g. `2:500ms` = retry twice, 500 ms between attempts). Bare `--retry` retries once
    /// after 500 ms. Does not multiply `--ping` samples.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "1:500ms",
        value_name = "COUNT|COUNT:INTERVAL",
        env = amaru::env_vars::PROBE_RETRY,
        display_order = 0,
    )]
    retry: Option<RetrySpec>,

    /// Highest N2N version to offer in the handshake (default: current Amaru N2N version).
    ///
    /// Proposes every Amaru-supported version from 11 through this value. The remote may
    /// Accept any offered version at or below this ceiling; a lower negotiated version is
    /// success, not a failure. Must be a version Amaru implements (11..=15 on this branch).
    /// Applies to the primary probe and to each `--ps-check` session.
    #[arg(
        long = "requested-version",
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::PROBE_REQUESTED_VERSION,
        default_value_t = VersionNumber::CURRENT.as_u64(),
        display_order = 0,
    )]
    requested_version: u64,

    /// Emit machine-readable JSON instead of color human output.
    #[arg(long, display_order = 0)]
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

/// Optional post-handshake tests for `--ps-check` (semicolon-separated).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PsCheckSpec {
    extras: session::PsCheckExtras,
}

impl PsCheckSpec {
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

impl FromStr for PsCheckSpec {
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
                        "unknown --ps-check test '{other}'; expected ping, tip, peer-share \
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

/// Run `op` once, then up to `retry.count` more times after `retry.interval` on failure.
async fn with_retries<T, E, F, Fut>(retry: Option<RetrySpec>, mut op: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let (extra, delay) = match retry {
        Some(spec) => (spec.count, spec.interval),
        None => (0, Duration::ZERO),
    };
    let mut attempt = 0u32;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(_err) if attempt < extra => {
                attempt += 1;
                tokio::time::sleep(delay).await;
            }
            Err(err) => return Err(err),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Actions {
    ping: Option<PingSpec>,
    handshake: bool,
    peer_share: bool,
    ps_check: Option<PsCheckSpec>,
    tip: bool,
}

impl Actions {
    fn from_args(args: &Args) -> Self {
        if args.all {
            return Self {
                ping: Some(PingSpec { count: 1, interval: Duration::from_secs(1) }),
                handshake: true,
                peer_share: true,
                ps_check: Some(PsCheckSpec::default()),
                tip: true,
            };
        }
        let mut actions = Self {
            ping: args.ping,
            handshake: args.handshake,
            peer_share: args.peer_share || args.ps_check.is_some(),
            ps_check: args.ps_check,
            tip: args.tip,
        };
        if actions.ping.is_none()
            && !(actions.handshake || actions.peer_share || actions.ps_check.is_some() || actions.tip)
        {
            // Default: full probe when no action flag is given.
            actions = Self {
                ping: Some(PingSpec { count: 1, interval: Duration::from_secs(1) }),
                handshake: true,
                peer_share: true,
                ps_check: None,
                tip: true,
            };
        }
        actions
    }

    fn needs_session(self) -> bool {
        self.ping.is_some() || self.peer_share || self.tip
    }
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let actions = Actions::from_args(&args);
    let peer: Peer = args.address.parse().context("invalid IP:PORT address")?;
    let connect_timeout = args.connect_timeout;
    let handshake_timeout = args.handshake_timeout;
    let protocol_timeout = args.protocol_timeout;
    let max_n2n_version = resolve_requested_version(args.requested_version)?;
    let color = color_enabled();
    let out = HumanOut::new(color);
    // Stream long-running human steps as they complete; JSON stays one final document.
    let stream = !args.json;

    info!(
        cli::peer::PROBE,
        address = args.address.as_str(),
        network = args.network,
        connect_timeout = duration::format(&args.connect_timeout),
        handshake_timeout = duration::format(&args.handshake_timeout),
        protocol_timeout = duration::format(&args.protocol_timeout),
        ping = args
            .ping
            .map(|p| format!("{}:{}", p.count, duration::format(&p.interval)))
            .unwrap_or_else(|| "off".to_string()),
        handshake = actions.handshake,
        peer_share = actions.peer_share,
        ps_check = actions.ps_check.map(|s| s.display()).unwrap_or_else(|| "off".to_string()),
        tip = actions.tip,
        all = args.all,
        time_track = args.time_track,
        retry = args
            .retry
            .map(|r| format!("{}:{}", r.count, duration::format(&r.interval)))
            .unwrap_or_else(|| "off".to_string()),
        requested_version = args.requested_version,
        json = args.json,
    );

    let mut report = session::ProbeReport {
        address: args.address.clone(),
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
        match with_retries(args.retry, || {
            session::run_session(session::SessionRequest {
                peer,
                network: args.network,
                connect_timeout,
                handshake_timeout,
                protocol_timeout,
                want_peershare: actions.peer_share,
                want_tip: actions.tip,
                want_ping: actions.ping.map(|s| (s.count, s.interval)),
                want_timetrack: args.time_track,
                timetrack_continue: track_clock,
                max_n2n_version,
                peer_share_amount: 10,
            })
        })
        .await
        {
            Ok(partial) => {
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
            if let Some(hs) = &report.handshake {
                out.handshake(hs);
            }
            if !report.ping_rtts_ms.is_empty() {
                out.ping(&report);
            }
            out.session_sections(&report, /*include_pscheck*/ false, /*skip_handshake*/ true);
        }
    } else if stream && actions.handshake {
        // handshake-only already streamed above
    }

    if stream && args.time_track && !report.timetrack.is_empty() {
        out.timetrack(&report.timetrack);
    }

    if let Some(ps_spec) = actions.ps_check {
        let peers = report.peers.clone().unwrap_or_default();
        let mut pscheck_header = stream && !peers.is_empty();
        let ps_params = PsCheckParams {
            network: args.network,
            connect_timeout,
            handshake_timeout,
            protocol_timeout,
            max_n2n_version,
            extras: ps_spec.extras,
            time_track: args.time_track,
        };
        for addr in peers {
            let check = match with_retries(args.retry, || run_pscheck_one(&addr, ps_params)).await {
                Ok(check) => check,
                Err(err) => session::PeerCheck::hard_failure(&addr, format!("{err:#}")),
            };
            if stream {
                if pscheck_header {
                    out.pscheck_header();
                    pscheck_header = false;
                }
                out.pscheck_nested(&check);
            }
            report.peer_checks.push(check);
        }
    }

    if args.json {
        emit(&report, true, color)?;
    } else if !stream {
        // Should not happen: stream is `!json`. Kept for clarity.
        emit(&report, false, color)?;
    }
    // Human streaming already printed each section as it completed.

    if !report.errors.is_empty() {
        bail!("probe completed with errors");
    }
    Ok(())
}

/// Shared probe settings applied to each `--ps-check` session.
#[derive(Debug, Clone, Copy)]
struct PsCheckParams {
    network: NetworkName,
    connect_timeout: Duration,
    handshake_timeout: Duration,
    protocol_timeout: Duration,
    max_n2n_version: VersionNumber,
    extras: session::PsCheckExtras,
    time_track: bool,
}

/// One TCP connect + handshake (+ optional extras). Hard connect/session errors are `Err`
/// so `--retry` can re-attempt; soft handshake/protocol failures are `Ok` with `ok: false`.
async fn run_pscheck_one(addr: &str, params: PsCheckParams) -> anyhow::Result<session::PeerCheck> {
    let Ok(peer) = addr.parse::<Peer>() else {
        // Not retryable: address came from peer-share encoding.
        return Ok(session::PeerCheck::hard_failure(addr, "invalid peer address from share"));
    };

    // Peer-share extras are reported on this check only; callers never enqueue those peers
    // for another ps-check (one discovery level).
    let partial = session::run_session(session::SessionRequest {
        peer,
        network: params.network,
        connect_timeout: params.connect_timeout,
        handshake_timeout: params.handshake_timeout,
        protocol_timeout: params.protocol_timeout,
        want_peershare: params.extras.peer_share,
        want_tip: params.extras.tip,
        want_ping: params.extras.ping.then_some((1, Duration::from_secs(1))),
        want_timetrack: params.time_track,
        timetrack_continue: None,
        max_n2n_version: params.max_n2n_version,
        peer_share_amount: 10,
    })
    .await?;

    Ok(session::PeerCheck::from_session(addr, partial, params.extras))
}

fn emit(report: &session::ProbeReport, json: bool, color: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        let out = HumanOut::new(color);
        out.probe_header(report);
        if let Some(hs) = &report.handshake {
            out.handshake(hs);
        }
        if !report.ping_rtts_ms.is_empty() {
            out.ping(report);
        }
        out.session_sections(report, /*include_pscheck*/ true, /*skip_handshake*/ true);
        if !report.timetrack.is_empty() {
            out.timetrack(&report.timetrack);
        }
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
        println!(
            "{}probe{} {}{}{} ({})",
            self.p.bold, self.p.reset, self.p.cyan, report.address, self.p.reset, report.network
        );
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

    /// Tip / peershare (and optionally already-collected pscheck rows).
    fn session_sections(&self, report: &session::ProbeReport, include_pscheck: bool, skip_handshake: bool) {
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
                    if include_pscheck && let Some(check) = report.peer_checks.iter().find(|c| c.address == *p) {
                        self.pscheck_nested(check);
                        continue;
                    }
                    println!("    {p}");
                }
            }
        }
        if include_pscheck {
            for check in &report.peer_checks {
                if report.peers.as_ref().is_some_and(|ps| ps.iter().any(|p| p == &check.address)) {
                    continue;
                }
                self.pscheck_orphan(check);
            }
        }
        self.errors(&report.errors);
        self.flush();
    }

    fn ping(&self, report: &session::ProbeReport) {
        let samples = &report.ping_rtts_ms;
        self.section_ok("ping");
        if samples.len() == 1 {
            self.field("    ", "rtt", format!("{} ms", format_rtt_ms(samples[0])));
            return;
        }
        let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let max = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let avg = samples.iter().sum::<f64>() / samples.len() as f64;
        self.field("    ", "count", samples.len());
        if let Some(interval) = report.ping_interval_ms {
            self.field("    ", "interval", format!("{interval} ms"));
        }
        for (i, ms) in samples.iter().enumerate() {
            self.field("    ", &(i + 1).to_string(), format!("{} ms", format_rtt_ms(*ms)));
        }
        self.field("    ", "min", format!("{} ms", format_rtt_ms(min)));
        self.field("    ", "avg", format!("{} ms", format_rtt_ms(avg)));
        self.field("    ", "max", format!("{} ms", format_rtt_ms(max)));
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

    fn pscheck_header(&self) {
        self.section_ok("ps-check");
        self.flush();
    }

    /// Nested under a peershare address, or a live streaming progress line.
    fn pscheck_nested(&self, check: &session::PeerCheck) {
        let tag = self.p.status(check.ok);
        println!("    {}  {tag}{}{}", check.address, format_pscheck_summary(check), self.p.reset);
        self.pscheck_details(check, "      ");
        self.flush();
    }

    fn pscheck_orphan(&self, check: &session::PeerCheck) {
        let tag = self.p.status(check.ok);
        println!("  {tag}ps-check{}  {}  {}", self.p.reset, check.address, format_pscheck_summary(check));
        self.pscheck_details(check, "           ");
    }

    fn pscheck_details(&self, check: &session::PeerCheck, indent: &str) {
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

fn format_pscheck_summary(check: &session::PeerCheck) -> String {
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
    fn ps_check_spec_bare_is_handshake_only() {
        let spec: PsCheckSpec = "".parse().unwrap();
        assert_eq!(spec.extras, session::PsCheckExtras::default());
        assert_eq!(spec.display(), "handshake");
    }

    #[test]
    fn ps_check_spec_parses_semicolon_extras() {
        let spec: PsCheckSpec = "ping;tip;peer-share".parse().unwrap();
        assert!(spec.extras.ping);
        assert!(spec.extras.tip);
        assert!(spec.extras.peer_share);
        assert_eq!(spec.display(), "ping;tip;peer-share");
    }

    #[test]
    fn ps_check_spec_rejects_unknown_token() {
        let err = "tip;foo".parse::<PsCheckSpec>().unwrap_err();
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
}
