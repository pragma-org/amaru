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

//! Open header spans, kept on the effect executor.
//!
//! `tracing::Span` is not serialized and does not enter the performance worker. The executor
//! opens and drops the spans; [`super::HeaderTelemetry::emit`] reads their durations.

use std::{collections::BTreeMap, time::SystemTime};

use amaru_kernel::HeaderHash;
use amaru_observability::{
    SchemaSpan, TraceContext,
    amaru::{
        consensus::roll_forward::PROCESS,
        network::perf::{
            blocks::FETCH,
            fork::SWITCH,
            header::{BLOCK_FETCH_WAIT, FORWARD},
        },
    },
    debug_span,
};

/// Durations taken from the spans, in microseconds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeaderDurations {
    pub forward_micros: Option<u64>,
    pub block_fetch_wait_micros: Option<u64>,
    pub block_fetch_micros: Option<u64>,
    pub adopt_micros: Option<u64>,
}

#[derive(Default)]
pub(crate) struct HeaderSpanBook {
    headers: BTreeMap<HeaderHash, OpenHeader>,
    fork: Option<OpenFork>,
}

struct OpenHeader {
    forward: Option<LiveSpan<FORWARD>>,
    context: TraceContext<FORWARD>,
    fetch_wait: Option<LiveSpan<BLOCK_FETCH_WAIT>>,
    fetch: Option<LiveSpan<FETCH>>,
    forward_micros: Option<u64>,
    block_fetch_wait_micros: Option<u64>,
    block_fetch_micros: Option<u64>,
    /// When the body span closed. Adoption is measured from here.
    body_received_at: Option<SystemTime>,
}

struct OpenFork {
    hash: HeaderHash,
    span: SchemaSpan<SWITCH>,
    opened_at: SystemTime,
}

struct LiveSpan<S> {
    span: SchemaSpan<S>,
    opened_at: SystemTime,
}

impl HeaderSpanBook {
    /// Open `perf.header.forward`. `parent` is the upstream roll-forward span; `None` is a root
    /// for a block this node forged. A second open of the same hash keeps the first span.
    pub(crate) fn open_forward(
        &mut self,
        hash: HeaderHash,
        parent: Option<TraceContext<PROCESS>>,
    ) -> TraceContext<FORWARD> {
        if let Some(existing) = self.headers.get(&hash) {
            return existing.context.clone();
        }
        let span = match &parent {
            Some(parent) => {
                debug_span!(parent_context: parent, network::perf::header::FORWARD, header_hash = hash)
            }
            None => debug_span!(root, network::perf::header::FORWARD, header_hash = hash),
        };
        let context = TraceContext::from(&span);
        self.headers.insert(
            hash,
            OpenHeader {
                forward: Some(LiveSpan { span, opened_at: SystemTime::now() }),
                context: context.clone(),
                fetch_wait: None,
                fetch: None,
                forward_micros: None,
                block_fetch_wait_micros: None,
                block_fetch_micros: None,
                body_received_at: None,
            },
        );
        context
    }

    pub(crate) fn open_fetch_wait(&mut self, hash: HeaderHash, parent: TraceContext<FORWARD>) {
        let Some(header) = self.headers.get(&hash) else {
            return;
        };
        if header.fetch_wait.is_some() {
            return;
        }
        let span = debug_span!(parent_context: &parent, network::perf::header::BLOCK_FETCH_WAIT, header_hash = hash);
        if let Some(header) = self.headers.get_mut(&hash) {
            header.fetch_wait = Some(LiveSpan { span, opened_at: SystemTime::now() });
        }
    }

    pub(crate) fn close_fetch_wait(&mut self, hash: &HeaderHash) {
        let Some(header) = self.headers.get_mut(hash) else {
            return;
        };
        if let Some(wait) = header.fetch_wait.take() {
            header.block_fetch_wait_micros = Some(close_live(wait, BLOCK_FETCH_WAIT::NAME, hash));
        }
    }

    /// Open one `perf.blocks.fetch` per header that still has a forward context.
    pub(crate) fn open_fetches(&mut self, hashes: &[HeaderHash]) {
        for hash in hashes {
            let Some(parent) = self.headers.get(hash).map(|header| header.context.clone()) else {
                continue;
            };
            if self.headers.get(hash).is_some_and(|header| header.fetch.is_some()) {
                continue;
            }
            let span = debug_span!(parent_context: &parent, network::perf::blocks::FETCH, header_hash = hash);
            if let Some(header) = self.headers.get_mut(hash) {
                header.fetch = Some(LiveSpan { span, opened_at: SystemTime::now() });
            }
        }
    }

    pub(crate) fn close_fetch(&mut self, hash: &HeaderHash) {
        let Some(header) = self.headers.get_mut(hash) else {
            return;
        };
        if let Some(fetch) = header.fetch.take() {
            header.block_fetch_micros = Some(close_live(fetch, FETCH::NAME, hash));
            header.body_received_at = Some(SystemTime::now());
        }
    }

    /// Drop the forward span now. Later lifecycle emission reads the stored duration.
    pub(crate) fn close_forward(&mut self, hash: &HeaderHash) {
        let Some(header) = self.headers.get_mut(hash) else {
            return;
        };
        if header.forward_micros.is_some() {
            return;
        }
        if let Some(forward) = header.forward.take() {
            header.forward_micros = Some(close_live(forward, FORWARD::NAME, hash));
        }
    }

    pub(crate) fn open_fork(&mut self, hash: &HeaderHash) {
        let Some(parent) = self.headers.get(hash).map(|header| header.context.clone()) else {
            return;
        };
        self.fork = Some(OpenFork {
            hash: *hash,
            span: debug_span!(parent_context: &parent, network::perf::fork::SWITCH, header_hash = hash),
            opened_at: SystemTime::now(),
        });
    }

    /// Close the fork span and return its duration.
    pub(crate) fn take_fork_micros(&mut self) -> Option<u64> {
        self.fork
            .take()
            .map(|fork| close_live(LiveSpan { span: fork.span, opened_at: fork.opened_at }, SWITCH::NAME, &fork.hash))
    }

    pub(crate) fn forward_span(&self, hash: &HeaderHash) -> Option<SchemaSpan<FORWARD>> {
        self.headers
            .get(hash)
            .and_then(|header| header.forward.as_ref().map(|live| SchemaSpan::from_span(tracing_span(&live.span))))
    }

    /// Close any span still open for `hash` and remove the entry.
    pub(crate) fn finish(&mut self, hash: &HeaderHash, record_adopt: bool) -> Option<HeaderDurations> {
        if let Some(header) = self.headers.get_mut(hash) {
            if let Some(wait) = header.fetch_wait.take() {
                header.block_fetch_wait_micros.get_or_insert(close_live(wait, BLOCK_FETCH_WAIT::NAME, hash));
            }
            if let Some(fetch) = header.fetch.take() {
                header.block_fetch_micros.get_or_insert(close_live(fetch, FETCH::NAME, hash));
            }
        }
        self.close_forward(hash);
        let header = self.headers.remove(hash)?;
        let adopt_micros = if record_adopt { header.body_received_at.map(elapsed_micros) } else { None };
        Some(HeaderDurations {
            forward_micros: header.forward_micros,
            block_fetch_wait_micros: header.block_fetch_wait_micros,
            block_fetch_micros: header.block_fetch_micros,
            adopt_micros,
        })
    }
}

fn tracing_span<S>(span: &SchemaSpan<S>) -> amaru_observability::tracing::Span {
    std::ops::Deref::deref(span).clone()
}

fn elapsed_micros(opened_at: SystemTime) -> u64 {
    SystemTime::now().duration_since(opened_at).unwrap_or_default().as_micros() as u64
}

fn close_live<S>(live: LiveSpan<S>, name: &str, hash: &HeaderHash) -> u64 {
    let fallback = elapsed_micros(live.opened_at);
    drop(live);
    amaru_observability::take_span_duration(name, &hash.to_string()).unwrap_or(fallback)
}
