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

//! External communication discipline: undirected session specs and projection
//! of typestate remainder graphs onto a binary session.
//!
//! [`SessionSpec`] stores the network-spec table (who sends). [`SessionSpec::project`]
//! orients that table for one [`Agency`]. Handler [`project`] hides mux plumbing,
//! timers, and local roles, unfolding remainder sequences onto a [`Cfsm`].
//!
//! Typestate remainder *use* lives in [`crate::typestate`].

#![expect(clippy::panic, clippy::unwrap_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Formatter, Write as _},
    time::Duration,
};

use crate::typestate::{EffectAst, InputName, Occupancy, PayloadName, RoleName, StateName, ThenAst, TypeGraph};

/// Who may send in a binary session (network-spec Client / Server).
#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
pub enum Agency {
    Initiator,
    Responder,
}

impl Agency {
    pub const fn opposite(self) -> Self {
        match self {
            Agency::Initiator => Agency::Responder,
            Agency::Responder => Agency::Initiator,
        }
    }
}

impl std::fmt::Display for Agency {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Agency::Initiator => write!(f, "initiator"),
            Agency::Responder => write!(f, "responder"),
        }
    }
}

/// Named typestate constructor, or a synthetic unfolding of a remainder sequence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum StateId {
    Named(StateName),
    Synthetic { parent: StateName, path: Vec<PayloadName> },
}

impl Display for StateId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            StateId::Named(name) => f.write_str(name),
            StateId::Synthetic { parent, path } => {
                f.write_str(parent)?;
                for p in path {
                    f.write_char('#')?;
                    f.write_str(p)?;
                }
                Ok(())
            }
        }
    }
}

/// Orientation of a projected label relative to the local role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Send,
    Recv,
}

impl Direction {
    fn opposite(self) -> Self {
        match self {
            Direction::Send => Direction::Recv,
            Direction::Recv => Direction::Send,
        }
    }

    fn as_bang_query(self) -> char {
        match self {
            Direction::Send => '!',
            Direction::Recv => '?',
        }
    }
}

/// Oriented message on a [`Cfsm`] edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Label {
    pub direction: Direction,
    pub message: &'static str,
}

impl Label {
    fn send(message: &'static str) -> Self {
        Self { direction: Direction::Send, message }
    }

    fn recv(message: &'static str) -> Self {
        Self { direction: Direction::Recv, message }
    }
}

impl Display for Label {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.direction.as_bang_query(), self.message)
    }
}

/// Oriented exclusive-agency machine. Timeouts live on [`SessionSpec`], not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cfsm {
    pub states: BTreeSet<StateId>,
    pub initial: StateId,
    pub terminal: BTreeSet<StateId>,
    /// Who may send. Omitted for [`terminal`](Self::terminal) states.
    pub agency: BTreeMap<StateId, Agency>,
    pub transitions: BTreeMap<StateId, BTreeMap<Label, StateId>>,
}

impl Cfsm {
    /// Panic on mismatch. Compares reachable oriented transitions, `agency`, and
    /// mapped `initial`. Does **not** compare timeouts.
    #[track_caller]
    pub fn assert_refines(&self, spec: &Cfsm, map: impl Fn(&StateId) -> StateId) {
        let got = self.reachable_fragment().collapse(map);
        let want = spec.reachable_fragment();
        if got.initial != want.initial || got.transitions != want.transitions || got.agency != want.agency {
            panic!("assert_refines mismatch\nprojected:\n{}\n\nspec:\n{}", got.fmt_table(), want.fmt_table());
        }
    }

    /// Reachable-table equality (transitions + agency + terminal). No search.
    #[track_caller]
    pub fn assert_bisimilar(&self, other: &Cfsm) {
        let got = self.reachable_fragment();
        let want = other.reachable_fragment();
        if got.initial != want.initial
            || got.transitions != want.transitions
            || got.agency != want.agency
            || got.terminal != want.terminal
        {
            panic!("assert_bisimilar mismatch\nleft:\n{}\n\nright:\n{}", got.fmt_table(), want.fmt_table());
        }
    }

    /// Swap `Send`/`Recv` only. **Copy** `agency` (who-sends) and `terminal`.
    ///
    /// `dual(project(I))` equals `project(R)` for exclusive-agency specs without
    /// [`SessionSpec::sim_open`]. Handshake-shaped specs omit the `sim_open` edge
    /// on the agency holder, so the two projections are not duals; do not
    /// [`assert_bisimilar`](Self::assert_bisimilar) them.
    #[must_use]
    pub fn dual(&self) -> Cfsm {
        let transitions = self
            .transitions
            .iter()
            .map(|(from, edges)| {
                let swapped = edges
                    .iter()
                    .map(|(label, to)| {
                        (Label { direction: label.direction.opposite(), message: label.message }, to.clone())
                    })
                    .collect();
                (from.clone(), swapped)
            })
            .collect();
        Cfsm {
            states: self.states.clone(),
            initial: self.initial.clone(),
            terminal: self.terminal.clone(),
            agency: self.agency.clone(),
            transitions,
        }
    }

    /// Apply a declared surjection; panic if two sources map to one dest with disagreeing destinations.
    #[must_use]
    #[track_caller]
    pub fn collapse(&self, map: impl Fn(&StateId) -> StateId) -> Cfsm {
        let mut states = BTreeSet::new();
        let mut transitions: BTreeMap<StateId, BTreeMap<Label, StateId>> = BTreeMap::new();
        let mut agency = BTreeMap::new();

        for s in &self.states {
            states.insert(map(s));
        }

        for (from, edges) in &self.transitions {
            let from2 = map(from);
            for (label, to) in edges {
                let to2 = map(to);
                states.insert(to2.clone());
                let slot = transitions.entry(from2.clone()).or_default();
                if let Some(existing) = slot.get(label)
                    && existing != &to2
                {
                    panic!(
                        "collapse: {from2} --{label}--> {to2} already defined as {existing} (disagreeing destinations)"
                    );
                }
                slot.insert(label.clone(), to2);
            }
        }

        for (s, role) in &self.agency {
            let s2 = map(s);
            if let Some(existing) = agency.get(&s2)
                && existing != role
            {
                panic!("collapse: {s2} agency {role} disagrees with existing {existing}");
            }
            agency.insert(s2, *role);
        }

        let mut out = Cfsm { states, initial: map(&self.initial), terminal: BTreeSet::new(), agency, transitions };
        out.recompute_terminal();
        out
    }

    /// Retarget every edge whose message is `done` to `new_to`. Other edges unchanged.
    #[must_use]
    pub fn retarget(&self, done: &str, new_to: StateId) -> Cfsm {
        let mut out = self.clone();
        out.states.insert(new_to.clone());
        for edges in out.transitions.values_mut() {
            for (label, to) in edges.iter_mut() {
                if label.message == done {
                    *to = new_to.clone();
                }
            }
        }
        out.recompute_terminal();
        out
    }

    /// Destination of the unique edge from `from` whose message is `msg`.
    #[track_caller]
    pub fn dest(&self, from: &StateId, msg: &str) -> StateId {
        let Some(edges) = self.transitions.get(from) else {
            panic!("dest: no transitions from {from}");
        };
        let mut found = None;
        for (label, to) in edges {
            if label.message == msg {
                if found.is_some() {
                    panic!("dest: multiple edges from {from} with message {msg:?}");
                }
                found = Some(to.clone());
            }
        }
        found.unwrap_or_else(|| panic!("dest: no edge from {from} with message {msg:?}"))
    }

    fn reachable(&self) -> BTreeSet<StateId> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![self.initial.clone()];
        while let Some(s) = stack.pop() {
            if !seen.insert(s.clone()) {
                continue;
            }
            if let Some(edges) = self.transitions.get(&s) {
                for to in edges.values() {
                    stack.push(to.clone());
                }
            }
        }
        seen
    }

    fn reachable_fragment(&self) -> Cfsm {
        let reach = self.reachable();
        let transitions =
            self.transitions.iter().filter(|(s, _)| reach.contains(s)).map(|(s, e)| (s.clone(), e.clone())).collect();
        let agency = self.agency.iter().filter(|(s, _)| reach.contains(s)).map(|(s, r)| (s.clone(), *r)).collect();
        let mut out =
            Cfsm { states: reach, initial: self.initial.clone(), terminal: BTreeSet::new(), agency, transitions };
        out.recompute_terminal();
        out
    }

    fn recompute_terminal(&mut self) {
        self.transitions.retain(|_, edges| !edges.is_empty());
        self.terminal = self
            .states
            .iter()
            .filter(|s| self.transitions.get(s).map(|e| e.is_empty()).unwrap_or(true))
            .cloned()
            .collect();
        self.agency.retain(|s, _| !self.terminal.contains(s));
    }

    fn fmt_table(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "initial: {}", self.initial);
        for state in &self.states {
            let agency = self.agency.get(state);
            let terminal = self.terminal.contains(state);
            let _ = writeln!(s, "{state} agency={agency:?} terminal={terminal}");
            if let Some(edges) = self.transitions.get(state) {
                for (label, to) in edges {
                    let _ = writeln!(s, "  {label} -> {to}");
                }
            }
        }
        s
    }
}

/// Undirected binary session: exclusive agency, labeled edges, optional timeouts.
///
/// The start state is [`start`](Self::start) if called, otherwise the `from` of
/// the first [`init`](Self::init) / [`resp`](Self::resp) / [`sim_open`](Self::sim_open).
/// Later builder calls do not change it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionSpec {
    pub(crate) transitions: BTreeMap<StateName, PerState>,
    /// Receiver's bound. Absence = no timer.
    timeout: BTreeMap<StateName, Duration>,
    /// Start state: `from` of the first `init` / `resp` / `sim_open` (not an explicit constructor).
    initial: Option<StateName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PerState {
    pub(crate) agency: Agency,
    pub(crate) transitions: BTreeMap<&'static str, Edge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Edge {
    pub(crate) to: StateName,
    pub(crate) sim_open: bool,
}

impl SessionSpec {
    /// Labels on undirected edges, in table order.
    pub fn edge_labels(&self) -> impl Iterator<Item = &'static str> {
        self.transitions.values().flat_map(|per| per.transitions.keys().copied())
    }

    /// Add a transition that the initiator sends.
    pub fn init(&mut self, from: StateName, msg: &'static str, to: StateName) {
        self.insert_edge(from, msg, to, Agency::Initiator, false);
    }

    /// Add a transition that the responder sends.
    pub fn resp(&mut self, from: StateName, msg: &'static str, to: StateName) {
        self.insert_edge(from, msg, to, Agency::Responder, false);
    }

    /// Simultaneous-open alias: Recv of `msg` for the waiting role, not mixed agency.
    pub fn sim_open(&mut self, from: StateName, msg: &'static str, to: StateName) {
        self.insert_edge(from, msg, to, Agency::Responder, true);
    }

    /// Set the start state. Used by [`session_spec!`](crate::session_spec) for `[*] --> S`.
    /// Later [`init`](Self::init) / [`resp`](Self::resp) / [`sim_open`](Self::sim_open)
    /// calls do not override it.
    pub fn start(&mut self, s: StateName) {
        self.initial = Some(s);
    }

    pub fn set_timeout(&mut self, state: StateName, d: Duration) {
        self.timeout.insert(state, d);
    }

    /// Receiver timeout for `state`, if any.
    pub fn timeout(&self, state: &str) -> Option<Duration> {
        self.timeout.get(state).copied()
    }

    fn insert_edge(&mut self, from: StateName, msg: &'static str, to: StateName, agency: Agency, sim_open: bool) {
        if self.initial.is_none() {
            self.initial = Some(from);
        }
        let per = self.transitions.entry(from).or_insert_with(|| PerState::role(agency));
        if let Some(present) = per.insert(msg, agency, to, sim_open) {
            panic!("transition {from:?} -> {msg:?} -> {present:?} already defined when inserting {to:?}");
        }
    }

    /// Panic on mismatch. Compares the undirected table after `map`.
    /// Does **not** compare timeouts or start state (`initial`).
    #[track_caller]
    pub fn assert_refines(&self, spec: &SessionSpec, map: impl Fn(&StateName) -> StateName) {
        let simplified = collapse_undirected(&self.transitions, map);
        assert_eq!(simplified, spec.transitions);
    }

    /// Library helper for protocols whose remainders still loop `MsgDone`.
    /// Retargets only the undirected done-edge destinations.
    #[must_use]
    pub fn with_restart_on_done(mut self, done: &'static str, to: StateName) -> Self {
        for per in self.transitions.values_mut() {
            for (msg, edge) in per.transitions.iter_mut() {
                if *msg == done {
                    edge.to = to;
                }
            }
        }
        self
    }

    /// Orient: from a state where `role == agency`, outgoing labels are Send; otherwise Recv.
    /// `sim_open` edges are Recv of the aliased message for the waiting role.
    pub fn project(&self, role: Agency) -> Cfsm {
        let Some(initial) = self.initial else {
            panic!("SessionSpec::project on empty spec");
        };
        let named = |s: StateName| StateId::Named(s);

        let mut states = BTreeSet::new();
        let mut transitions: BTreeMap<StateId, BTreeMap<Label, StateId>> = BTreeMap::new();
        let mut agency = BTreeMap::new();

        for (from, per) in &self.transitions {
            let from_id = named(*from);
            states.insert(from_id.clone());
            let mut edges = BTreeMap::new();
            for (msg, edge) in &per.transitions {
                let direction = if edge.sim_open {
                    if role == per.agency {
                        continue;
                    }
                    Direction::Recv
                } else if role == per.agency {
                    Direction::Send
                } else {
                    Direction::Recv
                };
                let to_id = named(edge.to);
                states.insert(to_id.clone());
                edges.insert(Label { direction, message: msg }, to_id);
            }
            if !edges.is_empty() {
                transitions.insert(from_id.clone(), edges);
                agency.insert(from_id, per.agency);
            }
        }

        let mut cfsm = Cfsm { states, initial: named(initial), terminal: BTreeSet::new(), agency, transitions };
        cfsm.recompute_terminal();
        cfsm
    }
}

impl PerState {
    fn role(agency: Agency) -> Self {
        Self { agency, transitions: BTreeMap::new() }
    }

    fn insert(&mut self, msg: &'static str, agency: Agency, to: StateName, sim_open: bool) -> Option<Edge> {
        assert_eq!(self.agency, agency, "inserting {msg:?}@{agency:?} to {to:?}");
        self.transitions.insert(msg, Edge { to, sim_open })
    }
}

#[track_caller]
fn collapse_undirected(
    transitions: &BTreeMap<StateName, PerState>,
    map: impl Fn(&StateName) -> StateName,
) -> BTreeMap<StateName, PerState> {
    let mut simplified = BTreeMap::<StateName, PerState>::new();
    for (from, per_state) in transitions {
        let from = map(from);
        for (message, edge) in &per_state.transitions {
            let to = map(&edge.to);
            let existing = simplified.entry(from).or_insert_with(|| PerState::role(per_state.agency)).insert(
                message,
                per_state.agency,
                to,
                edge.sim_open,
            );
            if let Some(existing) = existing.as_ref()
                && (existing.to != to || existing.sim_open != edge.sim_open)
            {
                let inserted = Edge { to, sim_open: edge.sim_open };
                panic!(
                    "transition {from:?} -> {message:?} already defined as {existing:?} when inserting {inserted:?}: disagreeing edge"
                );
            }
        }
    }
    simplified
}

/// Hand-written per protocol. `driven` selects the WantNext and timeout tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionConfig {
    pub role: Agency,
    pub peer_role: RoleName,
    pub mux_role: RoleName,
    pub local_roles: BTreeSet<RoleName>,
    /// Receive-arm identifiers that are mux wire messages (`stringify!($in)`).
    pub wire_inputs: BTreeSet<InputName>,
    /// Remainder `Call`/`Send` payload last-segments that are mux wire messages.
    pub wire_payload: BTreeSet<PayloadName>,
    pub plumbing_inputs: BTreeSet<InputName>,
    pub local_inputs: BTreeSet<InputName>,
    pub driven: bool,
}

/// Why mux projection of a remainder graph failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    ParallelWire { state: StateName, input: InputName },
    NoWireFromLocal { state: StateName, input: InputName },
    MixedHidableWireChoice { state: StateName, input: InputName },
    EmptyWireAlternatives { state: StateName, input: InputName },
    OverlappingInput { input: InputName },
    Nondeterministic { state: StateId, label: String },
    MixedAgency { state: StateId },
    AmbiguousRepeat { origin: StateId },
    RepeatStarTooWide { origin: StateId },
    PeerSendAny { role: RoleName },
    UnknownPeerPayload { payload: PayloadName },
    UnknownRole { role: RoleName },
    OccupancyDisagree { state: StateName },
    UnknownInput { state: StateName, input: InputName },
}

impl Display for ProjectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::ParallelWire { state, input } => {
                write!(f, "parallel wire effects in {state} + {input}")
            }
            ProjectError::NoWireFromLocal { state, input } => {
                write!(f, "local input {input} at {state} has no remaining wire send")
            }
            ProjectError::MixedHidableWireChoice { state, input } => {
                write!(f, "mixed hidable/wire choice in {state} + {input}")
            }
            ProjectError::EmptyWireAlternatives { state, input } => {
                write!(f, "wire input {input} at {state} has no alternatives")
            }
            ProjectError::OverlappingInput { input } => {
                write!(f, "input {input} is listed in more than one of plumbing, local, and wire")
            }
            ProjectError::Nondeterministic { state, label } => {
                write!(f, "nondeterministic {label} from {state}")
            }
            ProjectError::MixedAgency { state } => write!(f, "mixed Send and Recv at {state}"),
            ProjectError::AmbiguousRepeat { origin } => {
                write!(f, "repeat wire label equals the suffix's first wire label at {origin}")
            }
            ProjectError::RepeatStarTooWide { origin } => {
                write!(f, "repeat of more than one remaining wire effect at {origin}")
            }
            ProjectError::PeerSendAny { role } => write!(f, "SendAny to peer role {role}"),
            ProjectError::UnknownPeerPayload { payload } => {
                write!(f, "peer Call/Send payload {payload} is not in wire_payload")
            }
            ProjectError::UnknownRole { role } => write!(f, "role {role} is not mux, local, or peer"),
            ProjectError::OccupancyDisagree { state } => {
                write!(f, "declared occupancy disagrees with inferred agency at {state}")
            }
            ProjectError::UnknownInput { state, input } => {
                write!(f, "input {input} at {state} is not plumbing, local, or wire")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

/// Project a remainder graph onto the mux participant.
pub fn project(graph: &TypeGraph, cfg: &ProjectionConfig) -> Result<Cfsm, ProjectError> {
    if let Some(input) = overlapping_input(cfg) {
        return Err(ProjectError::OverlappingInput { input });
    }

    let mut proj = Projector { cfg, states: BTreeSet::new(), transitions: BTreeMap::new() };

    for name in &graph.states {
        proj.states.insert(StateId::Named(name));
    }

    for (state, inputs) in &graph.receives {
        for (input, rem) in inputs {
            if cfg.plumbing_inputs.contains(input) {
                continue;
            }

            let mut hidable_only = Vec::new();
            let mut wire_bearing = Vec::new();
            for alt in &rem.alternatives {
                let seq = proj.hide_parallel(state, input, alt)?;
                if seq.is_empty() {
                    hidable_only.push(alt);
                } else {
                    wire_bearing.push((seq, alt.next));
                }
            }

            if cfg.local_inputs.contains(input) {
                if !hidable_only.is_empty() && !wire_bearing.is_empty() {
                    return Err(ProjectError::MixedHidableWireChoice { state, input });
                }
                if wire_bearing.is_empty() {
                    return Err(ProjectError::NoWireFromLocal { state, input });
                }
                let origin = StateId::Named(state);
                for (seq, next) in wire_bearing {
                    proj.expand_seq(origin.clone(), &seq, next)?;
                }
                continue;
            }

            if cfg.wire_inputs.contains(input) {
                let m = *input;
                if !hidable_only.is_empty() && !wire_bearing.is_empty() {
                    return Err(ProjectError::MixedHidableWireChoice { state, input });
                }
                if hidable_only.is_empty() && wire_bearing.is_empty() {
                    return Err(ProjectError::EmptyWireAlternatives { state, input });
                }
                if !hidable_only.is_empty() {
                    let mut nexts = BTreeSet::new();
                    for alt in hidable_only {
                        nexts.insert(alt.next);
                    }
                    if nexts.len() != 1 {
                        return Err(ProjectError::Nondeterministic {
                            state: StateId::Named(state),
                            label: Label::recv(m).to_string(),
                        });
                    }
                    let next = *nexts.iter().next().unwrap();
                    proj.emit(StateId::Named(state), Label::recv(m), StateId::Named(next))?;
                } else {
                    let dest = next_synthetic(&StateId::Named(state), input);
                    proj.emit(StateId::Named(state), Label::recv(m), dest.clone())?;
                    for (seq, next) in wire_bearing {
                        proj.expand_seq(dest.clone(), &seq, next)?;
                    }
                }
                continue;
            }

            return Err(ProjectError::UnknownInput { state, input });
        }
    }

    let mut agency = BTreeMap::new();
    for state in &proj.states {
        let Some(edges) = proj.transitions.get(state) else {
            continue;
        };
        if edges.is_empty() {
            continue;
        }
        let mut has_send = false;
        let mut has_recv = false;
        for label in edges.keys() {
            match label.direction {
                Direction::Send => has_send = true,
                Direction::Recv => has_recv = true,
            }
        }
        let inferred = match (has_send, has_recv) {
            (true, true) => return Err(ProjectError::MixedAgency { state: state.clone() }),
            (true, false) => cfg.role,
            (false, true) => cfg.role.opposite(),
            (false, false) => continue,
        };
        agency.insert(state.clone(), inferred);
    }

    if !graph.occupancy.is_empty() {
        for (name, occ) in &graph.occupancy {
            let id = StateId::Named(name);
            let outgoing = proj.transitions.get(&id).is_some_and(|e| !e.is_empty());
            let inferred = agency.get(&id).copied();
            let ok = match occ {
                Occupancy::Terminal => !outgoing,
                Occupancy::Switch => outgoing && inferred == Some(cfg.role),
                Occupancy::Remote => outgoing && inferred == Some(cfg.role.opposite()),
            };
            if !ok {
                return Err(ProjectError::OccupancyDisagree { state: name });
            }
        }
    }

    let initial = StateId::Named(graph.initial);
    let mut cfsm =
        Cfsm { states: proj.states, initial, terminal: BTreeSet::new(), agency, transitions: proj.transitions };
    drop_unreachable(&mut cfsm);
    cfsm.recompute_terminal();
    Ok(cfsm)
}

/// Every receive arm is plumbing, local, or in `wire_inputs`. Every wire-map
/// label is an edge of `spec`.
#[track_caller]
pub fn assert_wire_inputs_cover_receives(graph: &TypeGraph, cfg: &ProjectionConfig, spec: &SessionSpec) {
    for (state, inputs) in &graph.receives {
        for input in inputs.keys() {
            if cfg.plumbing_inputs.contains(input) || cfg.local_inputs.contains(input) {
                continue;
            }
            assert!(cfg.wire_inputs.contains(input), "wire receive arm {input} at {state} is not in wire_inputs");
        }
    }
    let table: BTreeSet<&str> = spec.edge_labels().collect();
    for label in cfg.wire_inputs.iter().chain(cfg.wire_payload.iter()) {
        assert!(table.contains(label), "wire map label {label} is not in the session spec");
    }
}

struct Projector<'a> {
    cfg: &'a ProjectionConfig,
    states: BTreeSet<StateId>,
    transitions: BTreeMap<StateId, BTreeMap<Label, StateId>>,
}

impl Projector<'_> {
    fn hide_parallel<'b>(
        &self,
        state: StateName,
        input: InputName,
        alt: &'b ThenAst,
    ) -> Result<Vec<&'b EffectAst>, ProjectError> {
        if alt.parallel.is_empty() {
            return Ok(Vec::new());
        }
        let mut remaining = Vec::new();
        for branch in &alt.parallel {
            let seq = self.hide_seq(branch)?;
            if !seq.is_empty() {
                remaining.push(seq);
            }
        }
        match remaining.len() {
            0 => Ok(Vec::new()),
            1 => Ok(remaining.pop().unwrap()),
            _ => Err(ProjectError::ParallelWire { state, input }),
        }
    }

    fn hide_seq<'b>(&self, seq: &'b [EffectAst]) -> Result<Vec<&'b EffectAst>, ProjectError> {
        let mut out = Vec::new();
        for e in seq {
            if !self.is_hidable(e)? {
                out.push(e);
            }
        }
        Ok(out)
    }

    fn is_hidable(&self, e: &EffectAst) -> Result<bool, ProjectError> {
        match e {
            EffectAst::SetTimeout
            | EffectAst::ClearTimeout
            | EffectAst::Wait
            | EffectAst::Terminate
            | EffectAst::Clock
            | EffectAst::Schedule { .. }
            | EffectAst::CancelSchedule
            | EffectAst::External { .. }
            | EffectAst::AddStage => Ok(true),
            EffectAst::Send { role, payload } | EffectAst::Call { role, payload } => {
                if *role == self.cfg.peer_role {
                    if self.cfg.wire_payload.contains(payload) {
                        Ok(false)
                    } else {
                        Err(ProjectError::UnknownPeerPayload { payload })
                    }
                } else if *role == self.cfg.mux_role || self.cfg.local_roles.contains(role) {
                    Ok(true)
                } else {
                    Err(ProjectError::UnknownRole { role })
                }
            }
            EffectAst::SendAny { role } => {
                if *role == self.cfg.peer_role {
                    Err(ProjectError::PeerSendAny { role })
                } else if *role == self.cfg.mux_role || self.cfg.local_roles.contains(role) {
                    Ok(true)
                } else {
                    Err(ProjectError::UnknownRole { role })
                }
            }
            EffectAst::Repeat(body) => {
                for inner in body {
                    if !self.is_hidable(inner)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }

    /// `seq` is already hide-filtered (`Call` / `Send` / non-hidable `Repeat` only).
    fn expand_seq(&mut self, origin: StateId, seq: &[&EffectAst], named_next: StateName) -> Result<(), ProjectError> {
        assert!(!seq.is_empty(), "expand_seq on hide-filtered empty seq at {origin}");
        let mut state = origin;
        for (i, e) in seq.iter().copied().enumerate() {
            match e {
                EffectAst::Repeat(body) => {
                    let body = self.non_hidable_flat(body)?;
                    assert!(!body.is_empty(), "expand_seq Repeat body empty after hide at {state}");
                    if let Some(m) = self.single_wire_send(&body) {
                        if self.first_wire(&seq[i + 1..]).is_some_and(|head| head == m) {
                            return Err(ProjectError::AmbiguousRepeat { origin: state });
                        }
                        self.emit(state.clone(), Label::send(m), state.clone())?;
                    } else {
                        return Err(ProjectError::RepeatStarTooWide { origin: state });
                    }
                }
                EffectAst::Call { payload, .. } | EffectAst::Send { payload, .. } => {
                    if !self.cfg.wire_payload.contains(payload) {
                        panic!("expand_seq: payload {payload} missing from wire_payload at {state}");
                    }
                    let m = *payload;
                    if i + 1 == seq.len() {
                        self.emit(state, Label::send(m), StateId::Named(named_next))?;
                        return Ok(());
                    }
                    let dest = next_synthetic(&state, payload);
                    self.emit(state, Label::send(m), dest.clone())?;
                    state = dest;
                }
                EffectAst::SendAny { .. }
                | EffectAst::SetTimeout
                | EffectAst::ClearTimeout
                | EffectAst::Wait
                | EffectAst::Terminate
                | EffectAst::Clock
                | EffectAst::Schedule { .. }
                | EffectAst::CancelSchedule
                | EffectAst::External { .. }
                | EffectAst::AddStage => {
                    panic!("expand_seq: expected Call, Send, or Repeat after hide; got {e:?} at {state}")
                }
            }
        }
        Ok(())
    }

    fn non_hidable_flat<'b>(&self, body: &'b [EffectAst]) -> Result<Vec<&'b EffectAst>, ProjectError> {
        let mut flat = Vec::new();
        for e in body {
            if let EffectAst::Repeat(inner) = e {
                flat.extend(inner.iter());
            } else {
                flat.push(e);
            }
        }
        let mut out = Vec::new();
        for e in flat {
            if !self.is_hidable(e)? {
                out.push(e);
            }
        }
        Ok(out)
    }

    fn single_wire_send<'a>(&'a self, body: &'a [&EffectAst]) -> Option<&'static str> {
        match body {
            [e] => self.as_wire_send(e),
            _ => None,
        }
    }

    fn as_wire_send<'a>(&'a self, e: &'a EffectAst) -> Option<&'static str> {
        match e {
            EffectAst::Call { role, payload } | EffectAst::Send { role, payload } => {
                if *role == self.cfg.peer_role {
                    self.cfg.wire_payload.contains(payload).then_some(*payload)
                } else {
                    None
                }
            }
            EffectAst::SendAny { .. }
            | EffectAst::Repeat(_)
            | EffectAst::SetTimeout
            | EffectAst::ClearTimeout
            | EffectAst::Wait
            | EffectAst::Terminate
            | EffectAst::Clock
            | EffectAst::Schedule { .. }
            | EffectAst::CancelSchedule
            | EffectAst::External { .. }
            | EffectAst::AddStage => None,
        }
    }

    fn first_wire<'a>(&'a self, suffix: &'a [&EffectAst]) -> Option<&'static str> {
        suffix.iter().copied().find_map(|e| self.as_wire_send(e))
    }

    fn emit(&mut self, from: StateId, label: Label, to: StateId) -> Result<(), ProjectError> {
        self.states.insert(from.clone());
        self.states.insert(to.clone());
        let edges = self.transitions.entry(from.clone()).or_default();
        if let Some(existing) = edges.get(&label)
            && existing != &to
        {
            return Err(ProjectError::Nondeterministic { state: from, label: label.to_string() });
        }
        edges.insert(label, to);
        Ok(())
    }
}

fn next_synthetic(state: &StateId, payload: PayloadName) -> StateId {
    match state {
        StateId::Named(parent) => StateId::Synthetic { parent, path: vec![payload] },
        StateId::Synthetic { parent, path } => {
            let mut path = path.clone();
            path.push(payload);
            StateId::Synthetic { parent, path }
        }
    }
}

fn drop_unreachable(cfsm: &mut Cfsm) {
    let reach = cfsm.reachable();
    cfsm.states.retain(|s| reach.contains(s));
    cfsm.transitions.retain(|s, _| reach.contains(s));
    for edges in cfsm.transitions.values_mut() {
        edges.retain(|_, to| cfsm.states.contains(to));
    }
    cfsm.agency.retain(|s, _| cfsm.states.contains(s));
}

fn overlapping_input(cfg: &ProjectionConfig) -> Option<InputName> {
    for input in &cfg.plumbing_inputs {
        if cfg.local_inputs.contains(input) || cfg.wire_inputs.contains(input) {
            return Some(*input);
        }
    }
    for input in &cfg.local_inputs {
        if cfg.wire_inputs.contains(input) {
            return Some(*input);
        }
    }
    None
}

const WANT_NEXT_PAYLOAD: PayloadName = "WantNext";

/// Why [`check_want_next`] rejected a remainder graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WantNextError {
    WantNextInStar { state: StateName, input: InputName },
    WantNextMustBeSend { state: StateName, input: InputName },
    DuplicateWantNext { state: StateName, input: InputName },
    WantNextForbidden { state: StateName, input: InputName },
    WantNextMissing { state: StateName, input: InputName },
    DestNotSelf { state: StateName, input: InputName },
    MissingPull { dest: StateName },
    MissingOccupancy { state: StateName },
    UnlistedOccupancy { state: StateName, input: InputName },
    UnknownInput { state: StateName, input: InputName },
    WirePayloadMustBeCall { state: StateName, input: InputName, payload: PayloadName },
}

impl Display for WantNextError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WantNextError::WantNextInStar { state, input } => {
                write!(f, "WantNext inside Repeat at {state} + {input}")
            }
            WantNextError::WantNextMustBeSend { state, input } => {
                write!(f, "WantNext must be Send (not Call) at {state} + {input}")
            }
            WantNextError::DuplicateWantNext { state, input } => {
                write!(f, "more than one WantNext at {state} + {input}")
            }
            WantNextError::WantNextForbidden { state, input } => {
                write!(f, "WantNext forbidden at {state} + {input}")
            }
            WantNextError::WantNextMissing { state, input } => {
                write!(f, "WantNext required at {state} + {input}")
            }
            WantNextError::DestNotSelf { state, input } => {
                write!(f, "Pull WantNext dest must be self at {state} + {input}")
            }
            WantNextError::MissingPull { dest } => {
                write!(f, "missing Pull arm on {dest}")
            }
            WantNextError::MissingOccupancy { state } => {
                write!(f, "driven graph is missing occupancy for {state}")
            }
            WantNextError::UnlistedOccupancy { state, input } => {
                write!(f, "unlisted occupancy/kind at {state} + {input}")
            }
            WantNextError::UnknownInput { state, input } => {
                write!(f, "input {input} at {state} is not plumbing, local, or wire")
            }
            WantNextError::WirePayloadMustBeCall { state, input, payload } => {
                write!(f, "wire payload {payload} to peer must be Call (not Send) at {state} + {input}")
            }
        }
    }
}

impl std::error::Error for WantNextError {}

/// Why [`check_timeouts`] rejected a remainder graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeoutError {
    SetTimeoutForbidden { state: StateName, input: InputName },
    SetTimeoutMissing { state: StateName, input: InputName },
    ClearTimeoutMissing { state: StateName, input: InputName },
    MissingOccupancy { state: StateName },
    UnlistedOccupancy { state: StateName, input: InputName },
    UnknownInput { state: StateName, input: InputName },
}

impl Display for TimeoutError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeoutError::SetTimeoutForbidden { state, input } => {
                write!(f, "SetTimeout forbidden at {state} + {input}")
            }
            TimeoutError::SetTimeoutMissing { state, input } => {
                write!(f, "SetTimeout required at {state} + {input}")
            }
            TimeoutError::ClearTimeoutMissing { state, input } => {
                write!(f, "ClearTimeout required at {state} + {input}")
            }
            TimeoutError::MissingOccupancy { state } => {
                write!(f, "driven graph is missing occupancy for {state}")
            }
            TimeoutError::UnlistedOccupancy { state, input } => {
                write!(f, "unlisted occupancy/kind at {state} + {input}")
            }
            TimeoutError::UnknownInput { state, input } => {
                write!(f, "input {input} at {state} is not plumbing, local, or wire")
            }
        }
    }
}

impl std::error::Error for TimeoutError {}

#[derive(Clone, Copy)]
enum InputKind {
    Plumbing,
    Local,
    Wire,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    Required,
    Forbidden,
    Optional,
}

/// Mux `WantNext` well-formedness on the unprojected remainder graph.
///
/// `WantNext` is `Send<ToMux, WantNext>`, at most once per alternative, never
/// inside `Repeat`. Driven vs undriven tables follow occupancy / waiting states.
pub fn check_want_next(graph: &TypeGraph, cfg: &ProjectionConfig) -> Result<(), WantNextError> {
    if cfg.driven
        && let Err(state) = check_driven_occupancy(graph)
    {
        return Err(WantNextError::MissingOccupancy { state });
    }

    let mut need_pull = BTreeSet::new();

    for (state, inputs) in &graph.receives {
        for (input, rem) in inputs {
            let kind = input_kind(cfg, input);
            for alt in &rem.alternatives {
                let present = want_next_present(state, input, alt, cfg.mux_role)?;
                reject_peer_wire_send(state, input, alt, cfg)?;
                apply_want_next_rule(graph, cfg, state, input, kind, alt.next, present, &mut need_pull)?;
            }
        }
    }

    if cfg.driven {
        for dest in need_pull {
            if !has_pull_arm(graph, cfg, dest) {
                return Err(WantNextError::MissingPull { dest });
            }
        }
    } else if is_waiting(graph, cfg, graph.initial) && !has_pull_arm(graph, cfg, graph.initial) {
        return Err(WantNextError::MissingPull { dest: graph.initial });
    }

    Ok(())
}

/// Agency-timer presence on the unprojected remainder graph.
///
/// Looks up [`SessionSpec`] timeouts by typestate constructor name (`rem.next` /
/// occupancy dest). Duration values are not compared.
pub fn check_timeouts(graph: &TypeGraph, cfg: &ProjectionConfig, spec: &SessionSpec) -> Result<(), TimeoutError> {
    if cfg.driven
        && let Err(state) = check_driven_occupancy(graph)
    {
        return Err(TimeoutError::MissingOccupancy { state });
    }

    for (state, inputs) in &graph.receives {
        for (input, rem) in inputs {
            let kind = input_kind(cfg, input);
            for alt in &rem.alternatives {
                let timers = timer_presence(alt);
                let holds_agency = holds_peer_agency(alt, cfg.peer_role);
                apply_timeout_rule(graph, cfg, spec, state, input, kind, alt.next, timers, holds_agency)?;
            }
        }
    }
    Ok(())
}

fn input_kind(cfg: &ProjectionConfig, input: InputName) -> InputKind {
    if cfg.plumbing_inputs.contains(input) {
        InputKind::Plumbing
    } else if cfg.local_inputs.contains(input) {
        InputKind::Local
    } else if cfg.wire_inputs.contains(input) {
        InputKind::Wire
    } else {
        InputKind::Unknown
    }
}

fn occupancy_of(graph: &TypeGraph, name: StateName) -> Option<Occupancy> {
    graph.occupancy.get(name).copied()
}

fn check_driven_occupancy(graph: &TypeGraph) -> Result<(), StateName> {
    for name in &graph.states {
        occupancy_of(graph, name).ok_or(*name)?;
    }
    for inputs in graph.receives.values() {
        for rem in inputs.values() {
            for alt in &rem.alternatives {
                occupancy_of(graph, alt.next).ok_or(alt.next)?;
            }
        }
    }
    Ok(())
}

fn is_waiting(graph: &TypeGraph, cfg: &ProjectionConfig, state: StateName) -> bool {
    match occupancy_of(graph, state) {
        Some(Occupancy::Remote) => true,
        Some(Occupancy::Switch | Occupancy::Terminal) => false,
        None => graph.receives.get(state).is_some_and(|inputs| {
            inputs.keys().any(|input| cfg.plumbing_inputs.contains(input) || cfg.wire_inputs.contains(input))
        }),
    }
}

fn has_pull_arm(graph: &TypeGraph, cfg: &ProjectionConfig, state: StateName) -> bool {
    graph.receives.get(state).is_some_and(|inputs| inputs.keys().any(|input| cfg.plumbing_inputs.contains(input)))
}

fn want_next_present(state: StateName, input: InputName, alt: &ThenAst, mux: RoleName) -> Result<bool, WantNextError> {
    let mut scan = WantNextScan::default();
    for branch in &alt.parallel {
        scan_want_next(branch, mux, false, &mut scan);
    }
    if scan.in_star {
        return Err(WantNextError::WantNextInStar { state, input });
    }
    if scan.calls > 0 {
        return Err(WantNextError::WantNextMustBeSend { state, input });
    }
    if scan.sends > 1 {
        return Err(WantNextError::DuplicateWantNext { state, input });
    }
    Ok(scan.sends == 1)
}

fn reject_peer_wire_send(
    state: StateName,
    input: InputName,
    alt: &ThenAst,
    cfg: &ProjectionConfig,
) -> Result<(), WantNextError> {
    for branch in &alt.parallel {
        reject_peer_wire_send_seq(branch, state, input, cfg)?;
    }
    Ok(())
}

fn reject_peer_wire_send_seq(
    effects: &[EffectAst],
    state: StateName,
    input: InputName,
    cfg: &ProjectionConfig,
) -> Result<(), WantNextError> {
    for e in effects {
        match e {
            EffectAst::Repeat(body) => reject_peer_wire_send_seq(body, state, input, cfg)?,
            EffectAst::Send { role, payload } if *role == cfg.peer_role && cfg.wire_payload.contains(payload) => {
                return Err(WantNextError::WirePayloadMustBeCall { state, input, payload });
            }
            EffectAst::Send { .. }
            | EffectAst::Call { .. }
            | EffectAst::SendAny { .. }
            | EffectAst::SetTimeout
            | EffectAst::ClearTimeout
            | EffectAst::Wait
            | EffectAst::Terminate
            | EffectAst::Clock
            | EffectAst::Schedule { .. }
            | EffectAst::CancelSchedule
            | EffectAst::External { .. }
            | EffectAst::AddStage => {}
        }
    }
    Ok(())
}

#[derive(Default)]
struct WantNextScan {
    sends: usize,
    calls: usize,
    in_star: bool,
}

fn scan_want_next(effects: &[EffectAst], mux: RoleName, in_star: bool, scan: &mut WantNextScan) {
    for e in effects {
        match e {
            EffectAst::Repeat(body) => scan_want_next(body, mux, true, scan),
            EffectAst::Send { role, payload } if *role == mux && *payload == WANT_NEXT_PAYLOAD => {
                scan.sends += 1;
                scan.in_star |= in_star;
            }
            EffectAst::Call { role, payload } if *role == mux && *payload == WANT_NEXT_PAYLOAD => {
                scan.calls += 1;
                scan.in_star |= in_star;
            }
            EffectAst::Send { .. }
            | EffectAst::Call { .. }
            | EffectAst::SendAny { .. }
            | EffectAst::SetTimeout
            | EffectAst::ClearTimeout
            | EffectAst::Wait
            | EffectAst::Terminate
            | EffectAst::Clock
            | EffectAst::Schedule { .. }
            | EffectAst::CancelSchedule
            | EffectAst::External { .. }
            | EffectAst::AddStage => {}
        }
    }
}

#[expect(clippy::too_many_arguments)]
fn apply_want_next_rule(
    graph: &TypeGraph,
    cfg: &ProjectionConfig,
    state: StateName,
    input: InputName,
    kind: InputKind,
    next: StateName,
    present: bool,
    need_pull: &mut BTreeSet<StateName>,
) -> Result<(), WantNextError> {
    if matches!(kind, InputKind::Unknown) {
        return Err(WantNextError::UnknownInput { state, input });
    }
    let (rule, dest_self) = want_next_rule(graph, cfg, state, input, kind, next, need_pull)?;
    match (rule, present) {
        (Presence::Required, false) => Err(WantNextError::WantNextMissing { state, input }),
        (Presence::Forbidden, true) => Err(WantNextError::WantNextForbidden { state, input }),
        (Presence::Required, true) if dest_self && next != state => Err(WantNextError::DestNotSelf { state, input }),
        _ => Ok(()),
    }
}

fn want_next_rule(
    graph: &TypeGraph,
    cfg: &ProjectionConfig,
    state: StateName,
    input: InputName,
    kind: InputKind,
    next: StateName,
    need_pull: &mut BTreeSet<StateName>,
) -> Result<(Presence, bool), WantNextError> {
    if cfg.driven {
        driven_want_next_rule(graph, state, input, kind, next, need_pull)
    } else {
        Ok(undriven_want_next_rule(graph, cfg, kind, next))
    }
}

fn driven_want_next_rule(
    graph: &TypeGraph,
    state: StateName,
    input: InputName,
    kind: InputKind,
    next: StateName,
    need_pull: &mut BTreeSet<StateName>,
) -> Result<(Presence, bool), WantNextError> {
    let Some(src) = occupancy_of(graph, state) else {
        return Err(WantNextError::MissingOccupancy { state });
    };
    let Some(dst) = occupancy_of(graph, next) else {
        return Err(WantNextError::MissingOccupancy { state: next });
    };
    if dst == Occupancy::Terminal {
        return Ok((Presence::Forbidden, false));
    }
    match (src, dst, kind) {
        (Occupancy::Switch, Occupancy::Remote, InputKind::Local) => {
            need_pull.insert(next);
            Ok((Presence::Forbidden, false))
        }
        (Occupancy::Remote, Occupancy::Remote, InputKind::Plumbing) => Ok((Presence::Required, true)),
        (Occupancy::Remote, Occupancy::Remote, InputKind::Wire) => Ok((Presence::Required, false)),
        (Occupancy::Remote, Occupancy::Switch, InputKind::Wire) => Ok((Presence::Forbidden, false)),
        _ => Err(WantNextError::UnlistedOccupancy { state, input }),
    }
}

fn undriven_want_next_rule(
    graph: &TypeGraph,
    cfg: &ProjectionConfig,
    kind: InputKind,
    next: StateName,
) -> (Presence, bool) {
    let dest_self = matches!(kind, InputKind::Plumbing);
    if dest_self || is_waiting(graph, cfg, next) {
        (Presence::Required, dest_self)
    } else {
        (Presence::Optional, dest_self)
    }
}

struct TimerPresence {
    set: bool,
    clear: bool,
}

fn timer_presence(alt: &ThenAst) -> TimerPresence {
    let mut set = false;
    let mut clear = false;
    for branch in &alt.parallel {
        scan_timers(branch, &mut set, &mut clear);
    }
    TimerPresence { set, clear }
}

fn scan_timers(effects: &[EffectAst], set: &mut bool, clear: &mut bool) {
    for e in effects {
        match e {
            EffectAst::SetTimeout => *set = true,
            EffectAst::ClearTimeout => *clear = true,
            // Timers inside Repeat are not present: zero iterations never arm or clear.
            EffectAst::Repeat(_)
            | EffectAst::Send { .. }
            | EffectAst::Call { .. }
            | EffectAst::SendAny { .. }
            | EffectAst::Wait
            | EffectAst::Terminate
            | EffectAst::Clock
            | EffectAst::Schedule { .. }
            | EffectAst::CancelSchedule
            | EffectAst::External { .. }
            | EffectAst::AddStage => {}
        }
    }
}

#[expect(clippy::too_many_arguments)]
fn apply_timeout_rule(
    graph: &TypeGraph,
    cfg: &ProjectionConfig,
    spec: &SessionSpec,
    state: StateName,
    input: InputName,
    kind: InputKind,
    next: StateName,
    timers: TimerPresence,
    holds_agency: bool,
) -> Result<(), TimeoutError> {
    if matches!(kind, InputKind::Unknown) {
        return Err(TimeoutError::UnknownInput { state, input });
    }
    let (set_rule, clear_rule) = if cfg.driven {
        driven_timeout_rules(graph, spec, state, input, kind, next)?
    } else {
        undriven_timeout_rules(graph, cfg, spec, next, holds_agency)
    };
    match (set_rule, timers.set) {
        (Presence::Required, false) => {
            return Err(TimeoutError::SetTimeoutMissing { state, input });
        }
        (Presence::Forbidden, true) => {
            return Err(TimeoutError::SetTimeoutForbidden { state, input });
        }
        _ => {}
    }
    match (clear_rule, timers.clear) {
        (Presence::Required, false) => Err(TimeoutError::ClearTimeoutMissing { state, input }),
        _ => Ok(()),
    }
}

fn driven_timeout_rules(
    graph: &TypeGraph,
    spec: &SessionSpec,
    state: StateName,
    input: InputName,
    kind: InputKind,
    next: StateName,
) -> Result<(Presence, Presence), TimeoutError> {
    let Some(src) = occupancy_of(graph, state) else {
        return Err(TimeoutError::MissingOccupancy { state });
    };
    let Some(dst) = occupancy_of(graph, next) else {
        return Err(TimeoutError::MissingOccupancy { state: next });
    };
    if dst == Occupancy::Terminal {
        return Ok((Presence::Forbidden, Presence::Optional));
    }
    let dest_timed = spec.timeout(next).is_some();
    let src_timed = spec.timeout(state).is_some();
    match (src, dst, kind) {
        (Occupancy::Switch, Occupancy::Remote, InputKind::Local) => Ok((Presence::Forbidden, Presence::Optional)),
        (Occupancy::Remote, Occupancy::Remote, InputKind::Plumbing | InputKind::Wire) => {
            let set = if dest_timed { Presence::Required } else { Presence::Forbidden };
            Ok((set, Presence::Optional))
        }
        (Occupancy::Remote, Occupancy::Switch, InputKind::Wire) => {
            let clear = if src_timed { Presence::Required } else { Presence::Optional };
            Ok((Presence::Forbidden, clear))
        }
        _ => Err(TimeoutError::UnlistedOccupancy { state, input }),
    }
}

fn undriven_timeout_rules(
    graph: &TypeGraph,
    cfg: &ProjectionConfig,
    spec: &SessionSpec,
    next: StateName,
    holds_agency: bool,
) -> (Presence, Presence) {
    // SetTimeout only if we wait at `next` and do not hold local agency along the remainder.
    let set = if is_waiting(graph, cfg, next) && !holds_agency && spec.timeout(next).is_some() {
        Presence::Required
    } else {
        Presence::Forbidden
    };
    (set, Presence::Optional)
}

fn holds_peer_agency(alt: &ThenAst, peer: RoleName) -> bool {
    alt.parallel.iter().any(|branch| seq_holds_peer(branch, peer))
}

fn seq_holds_peer(effects: &[EffectAst], peer: RoleName) -> bool {
    effects.iter().any(|e| match e {
        EffectAst::Call { role, .. } | EffectAst::Send { role, .. } | EffectAst::SendAny { role } if *role == peer => {
            true
        }
        EffectAst::Repeat(body) => seq_holds_peer(body, peer),
        EffectAst::Call { .. }
        | EffectAst::Send { .. }
        | EffectAst::SendAny { .. }
        | EffectAst::SetTimeout
        | EffectAst::ClearTimeout
        | EffectAst::Wait
        | EffectAst::Terminate
        | EffectAst::Clock
        | EffectAst::Schedule { .. }
        | EffectAst::CancelSchedule
        | EffectAst::External { .. }
        | EffectAst::AddStage => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typestate::RemainderAst;

    fn call(role: RoleName, payload: PayloadName) -> EffectAst {
        EffectAst::Call { role, payload }
    }

    fn send(role: RoleName, payload: PayloadName) -> EffectAst {
        EffectAst::Send { role, payload }
    }

    fn send_any(role: RoleName) -> EffectAst {
        EffectAst::SendAny { role }
    }

    fn repeat(body: Vec<EffectAst>) -> EffectAst {
        EffectAst::Repeat(body)
    }

    fn seq(effects: Vec<EffectAst>, next: StateName) -> RemainderAst {
        RemainderAst { alternatives: vec![ThenAst { parallel: vec![effects], next }] }
    }

    fn choice(alts: Vec<ThenAst>) -> RemainderAst {
        RemainderAst { alternatives: alts }
    }

    fn then_seq(effects: Vec<EffectAst>, next: StateName) -> ThenAst {
        ThenAst { parallel: vec![effects], next }
    }

    fn payloads() -> BTreeSet<PayloadName> {
        BTreeSet::from(["RequestRange", "ClientDone", "StartBatch", "NoBlocks", "Block", "BatchDone"])
    }

    fn table_37() -> SessionSpec {
        let mut spec = SessionSpec::default();
        spec.init("Idle", "RequestRange", "Busy");
        spec.init("Idle", "ClientDone", "Done");
        spec.resp("Busy", "NoBlocks", "Idle");
        spec.resp("Busy", "StartBatch", "Streaming");
        spec.resp("Streaming", "Block", "Streaming");
        spec.resp("Streaming", "BatchDone", "Idle");
        spec.set_timeout("Busy", Duration::from_secs(60));
        spec.set_timeout("Streaming", Duration::from_secs(60));
        spec
    }

    fn cfg_initiator() -> ProjectionConfig {
        ProjectionConfig {
            role: Agency::Initiator,
            peer_role: "ToResponder",
            mux_role: "ToMux",
            local_roles: BTreeSet::from(["ToCollector"]),
            wire_inputs: BTreeSet::from(["StartBatch", "NoBlocks", "Block", "BatchDone"]),
            wire_payload: payloads(),
            plumbing_inputs: BTreeSet::from(["Pull"]),
            local_inputs: BTreeSet::from(["Fetch", "Close"]),
            driven: true,
        }
    }

    fn cfg_responder() -> ProjectionConfig {
        ProjectionConfig {
            role: Agency::Responder,
            peer_role: "ToInitiator",
            mux_role: "ToMux",
            local_roles: BTreeSet::new(),
            wire_inputs: BTreeSet::from(["RequestRange", "ClientDone"]),
            wire_payload: payloads(),
            plumbing_inputs: BTreeSet::from(["Pull"]),
            local_inputs: BTreeSet::new(),
            driven: false,
        }
    }

    fn initiator_graph() -> TypeGraph {
        TypeGraph {
            states: BTreeSet::from(["Idle", "Busy", "Streaming", "Done"]),
            initial: "Idle",
            occupancy: BTreeMap::from([
                ("Idle", Occupancy::Switch),
                ("Busy", Occupancy::Remote),
                ("Streaming", Occupancy::Remote),
                ("Done", Occupancy::Terminal),
            ]),
            receives: BTreeMap::from([
                (
                    "Idle",
                    BTreeMap::from([
                        ("Fetch", seq(vec![call("ToResponder", "RequestRange")], "Busy")),
                        (
                            "Close",
                            RemainderAst {
                                alternatives: vec![ThenAst {
                                    parallel: vec![
                                        vec![call("ToResponder", "ClientDone")],
                                        vec![repeat(vec![send_any("ToCollector")])],
                                    ],
                                    next: "Done",
                                }],
                            },
                        ),
                    ]),
                ),
                (
                    "Busy",
                    BTreeMap::from([
                        ("Pull", seq(vec![send("ToMux", "WantNext"), EffectAst::SetTimeout], "Busy")),
                        ("StartBatch", seq(vec![send("ToMux", "WantNext"), EffectAst::SetTimeout], "Streaming")),
                        ("NoBlocks", seq(vec![EffectAst::ClearTimeout, repeat(vec![send_any("ToCollector")])], "Idle")),
                    ]),
                ),
                (
                    "Streaming",
                    BTreeMap::from([
                        (
                            "Block",
                            seq(
                                vec![
                                    send("ToMux", "WantNext"),
                                    repeat(vec![send_any("ToCollector")]),
                                    EffectAst::SetTimeout,
                                ],
                                "Streaming",
                            ),
                        ),
                        (
                            "BatchDone",
                            seq(vec![EffectAst::ClearTimeout, repeat(vec![send_any("ToCollector")])], "Idle"),
                        ),
                    ]),
                ),
                ("Done", BTreeMap::new()),
            ]),
        }
    }

    fn responder_graph() -> TypeGraph {
        TypeGraph {
            states: BTreeSet::from(["Idle", "Done"]),
            initial: "Idle",
            occupancy: BTreeMap::new(),
            receives: BTreeMap::from([
                (
                    "Idle",
                    BTreeMap::from([
                        ("Pull", seq(vec![send("ToMux", "WantNext")], "Idle")),
                        (
                            "RequestRange",
                            choice(vec![
                                then_seq(
                                    vec![
                                        call("ToInitiator", "StartBatch"),
                                        repeat(vec![call("ToInitiator", "Block")]),
                                        call("ToInitiator", "BatchDone"),
                                        send("ToMux", "WantNext"),
                                    ],
                                    "Idle",
                                ),
                                then_seq(vec![call("ToInitiator", "NoBlocks"), send("ToMux", "WantNext")], "Idle"),
                            ]),
                        ),
                        ("ClientDone", seq(vec![send("ToMux", "WantNext")], "Done")),
                    ]),
                ),
                ("Done", BTreeMap::new()),
            ]),
        }
    }

    fn named(s: StateName) -> StateId {
        StateId::Named(s)
    }

    fn identity(id: &StateId) -> StateId {
        id.clone()
    }

    fn map_responder(id: &StateId) -> StateId {
        match id {
            StateId::Named(_) => id.clone(),
            StateId::Synthetic { parent: "Idle", path } if path.as_slice() == ["RequestRange"] => named("Busy"),
            StateId::Synthetic { parent: "Idle", path } if path.as_slice() == ["RequestRange", "StartBatch"] => {
                named("Streaming")
            }
            other => panic!("unmapped responder state {other}"),
        }
    }

    fn idle_graph(receives: BTreeMap<InputName, RemainderAst>) -> TypeGraph {
        TypeGraph {
            states: BTreeSet::from(["Idle", "Done"]),
            initial: "Idle",
            occupancy: BTreeMap::new(),
            receives: BTreeMap::from([("Idle", receives)]),
        }
    }

    #[test]
    fn initiator_graph_projects_to_table_37() {
        let spec = table_37().project(Agency::Initiator);
        let got = project(&initiator_graph(), &cfg_initiator()).unwrap();
        assert!(got.states.iter().all(|s| matches!(s, StateId::Named(_))));
        assert_eq!(got.initial, named("Idle"));
        got.assert_refines(&spec, identity);
        assert_eq!(got.agency.get(&named("Idle")), Some(&Agency::Initiator));
        assert_eq!(got.agency.get(&named("Busy")), Some(&Agency::Responder));
        assert_eq!(got.agency.get(&named("Streaming")), Some(&Agency::Responder));
        assert!(got.terminal.contains(&named("Done")));
        assert_eq!(got.dest(&named("Idle"), "RequestRange"), named("Busy"));
        assert_eq!(got.dest(&named("Idle"), "ClientDone"), named("Done"));
        assert_eq!(got.dest(&named("Busy"), "NoBlocks"), named("Idle"));
        assert_eq!(got.dest(&named("Busy"), "StartBatch"), named("Streaming"));
        assert_eq!(got.dest(&named("Streaming"), "Block"), named("Streaming"));
        assert_eq!(got.dest(&named("Streaming"), "BatchDone"), named("Idle"));
    }

    #[test]
    fn responder_request_range_synthetics_refine_spec() {
        let spec = table_37().project(Agency::Responder);
        let got = project(&responder_graph(), &cfg_responder()).unwrap();
        let req = StateId::Synthetic { parent: "Idle", path: vec!["RequestRange"] };
        let start = StateId::Synthetic { parent: "Idle", path: vec!["RequestRange", "StartBatch"] };
        assert_eq!(got.dest(&named("Idle"), "RequestRange"), req);
        assert_eq!(got.dest(&req, "StartBatch"), start.clone());
        assert_eq!(got.dest(&req, "NoBlocks"), named("Idle"));
        assert_eq!(got.dest(&start, "Block"), start.clone());
        assert_eq!(got.dest(&start, "BatchDone"), named("Idle"));
        assert_eq!(got.dest(&named("Idle"), "ClientDone"), named("Done"));
        got.assert_refines(&spec, map_responder);
        got.collapse(map_responder).assert_bisimilar(&spec);
    }

    #[test]
    fn sim_open_is_recv_for_waiting_role_only() {
        let mut spec = SessionSpec::default();
        spec.init("Propose", "Propose", "Confirm");
        spec.sim_open("Confirm", "Propose", "Done");
        spec.resp("Confirm", "Accept", "Done");
        spec.resp("Confirm", "Refuse", "Done");
        spec.resp("Confirm", "QueryReply", "Done");

        let spec_i = spec.project(Agency::Initiator);
        assert_eq!(spec_i.initial, named("Propose"));
        assert_eq!(spec_i.dest(&named("Propose"), "Propose"), named("Confirm"));
        assert_eq!(spec_i.dest(&named("Confirm"), "Propose"), named("Done"));
        assert_eq!(spec_i.dest(&named("Confirm"), "Accept"), named("Done"));
        let confirm_i = spec_i.transitions.get(&named("Confirm")).unwrap();
        assert!(confirm_i.keys().all(|l| l.direction == Direction::Recv));
        assert_eq!(spec_i.agency.get(&named("Confirm")), Some(&Agency::Responder));

        let spec_r = spec.project(Agency::Responder);
        assert_eq!(spec_r.initial, named("Propose"));
        assert_eq!(spec_r.dest(&named("Propose"), "Propose"), named("Confirm"));
        assert_eq!(spec_r.dest(&named("Confirm"), "Accept"), named("Done"));
        assert_eq!(spec_r.dest(&named("Confirm"), "Refuse"), named("Done"));
        assert_eq!(spec_r.dest(&named("Confirm"), "QueryReply"), named("Done"));
        let confirm_r = spec_r.transitions.get(&named("Confirm")).unwrap();
        assert!(confirm_r.keys().all(|l| l.direction == Direction::Send));
        assert!(confirm_r.keys().all(|l| l.message != "Propose"));
        assert_eq!(spec_r.agency.get(&named("Confirm")), Some(&Agency::Responder));
        // Handshake is not dual(project(I)) == project(R): sim_open is Recv for
        // the waiting role and omitted for the agency holder.
    }

    #[test]
    fn dual_of_spec_initiator_equals_spec_responder() {
        // Table 3.7 has exclusive agency and no sim_open; duality holds only then.
        let spec = table_37();
        let spec_i = spec.project(Agency::Initiator);
        let spec_r = spec.project(Agency::Responder);
        spec_i.dual().assert_bisimilar(&spec_r);
        assert_eq!(spec_i.agency.get(&named("Idle")), Some(&Agency::Initiator));
        assert_eq!(spec_r.agency.get(&named("Idle")), Some(&Agency::Initiator));
        assert_eq!(spec_i.dual().agency.get(&named("Idle")), Some(&Agency::Initiator));
        assert_eq!(spec_i.agency.get(&named("Busy")), Some(&Agency::Responder));
        assert_eq!(spec_r.agency.get(&named("Busy")), Some(&Agency::Responder));
    }

    #[test]
    fn with_restart_on_done_retargets_only_the_done_edge() {
        let spec = table_37().with_restart_on_done("ClientDone", "Idle");
        let cfsm = spec.project(Agency::Initiator);
        assert_eq!(cfsm.dest(&named("Idle"), "ClientDone"), named("Idle"));
        assert_eq!(cfsm.dest(&named("Idle"), "RequestRange"), named("Busy"));
        assert_eq!(cfsm.dest(&named("Busy"), "StartBatch"), named("Streaming"));
        assert_eq!(spec.timeout("Busy"), Some(Duration::from_secs(60)));
        let original = table_37().project(Agency::Initiator);
        assert_eq!(original.dest(&named("Idle"), "ClientDone"), named("Done"));
        let retargeted = original.retarget("ClientDone", named("Idle"));
        assert_eq!(retargeted.dest(&named("Idle"), "ClientDone"), named("Idle"));
        assert_eq!(retargeted.dest(&named("Idle"), "RequestRange"), named("Busy"));
    }

    #[test]
    fn assert_refines_ignores_timeouts() {
        let mut timed = table_37();
        timed.set_timeout("Idle", Duration::from_secs(1));
        timed.project(Agency::Initiator).assert_refines(&table_37().project(Agency::Initiator), identity);
        timed.assert_refines(&table_37(), |s| *s);
    }

    #[test]
    fn undirected_assert_refines_preserves_sim_open() {
        let mut spec = SessionSpec::default();
        spec.init("Propose", "Propose", "Confirm");
        spec.sim_open("Confirm", "Propose", "Done");
        spec.resp("Confirm", "Accept", "Done");
        spec.assert_refines(&spec, |s| *s);
    }

    #[test]
    #[should_panic(expected = "disagreeing edge")]
    fn undirected_assert_refines_panics_on_disagreeing_sim_open() {
        let mut got = SessionSpec::default();
        got.resp("ConfirmA", "Propose", "Done");
        got.sim_open("ConfirmB", "Propose", "Done");

        let mut want = SessionSpec::default();
        want.sim_open("Confirm", "Propose", "Done");

        got.assert_refines(&want, |s| match *s {
            "ConfirmA" | "ConfirmB" => "Confirm",
            other => other,
        });
    }

    #[test]
    fn repeat_block_then_batch_done_is_not_ambiguous() {
        let graph = idle_graph(BTreeMap::from([(
            "RequestRange",
            seq(
                vec![
                    repeat(vec![call("ToInitiator", "Block")]),
                    EffectAst::SetTimeout,
                    call("ToInitiator", "BatchDone"),
                ],
                "Idle",
            ),
        )]));
        let got = project(&graph, &cfg_responder()).unwrap();
        let syn = StateId::Synthetic { parent: "Idle", path: vec!["RequestRange"] };
        assert_eq!(got.dest(&syn, "Block"), syn.clone());
        assert_eq!(got.dest(&syn, "BatchDone"), named("Idle"));
    }

    #[test]
    fn projection_errors() {
        struct Case {
            name: &'static str,
            graph: TypeGraph,
            cfg: ProjectionConfig,
            check: fn(&ProjectError) -> bool,
        }

        let cases = [
            Case {
                name: "ParallelWire",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    RemainderAst {
                        alternatives: vec![ThenAst {
                            parallel: vec![
                                vec![call("ToResponder", "RequestRange")],
                                vec![call("ToResponder", "ClientDone")],
                            ],
                            next: "Done",
                        }],
                    },
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::ParallelWire { state: "Idle", input: "Fetch" }),
            },
            Case {
                name: "RepeatStarTooWide",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    seq(
                        vec![repeat(vec![call("ToResponder", "RequestRange"), call("ToResponder", "ClientDone")])],
                        "Done",
                    ),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::RepeatStarTooWide { .. }),
            },
            Case {
                name: "AmbiguousRepeat",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    seq(
                        vec![repeat(vec![call("ToResponder", "RequestRange")]), call("ToResponder", "RequestRange")],
                        "Busy",
                    ),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::AmbiguousRepeat { .. }),
            },
            Case {
                name: "MixedHidableWireChoice",
                graph: idle_graph(BTreeMap::from([(
                    "RequestRange",
                    choice(vec![
                        then_seq(vec![send("ToMux", "WantNext")], "Idle"),
                        then_seq(vec![call("ToInitiator", "NoBlocks")], "Idle"),
                    ]),
                )])),
                cfg: cfg_responder(),
                check: |e| matches!(e, ProjectError::MixedHidableWireChoice { state: "Idle", input: "RequestRange" }),
            },
            Case {
                name: "NoWireFromLocal",
                graph: idle_graph(BTreeMap::from([("Fetch", seq(vec![send("ToMux", "WantNext")], "Idle"))])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::NoWireFromLocal { state: "Idle", input: "Fetch" }),
            },
            Case {
                name: "NondeterministicHidableNexts",
                graph: idle_graph(BTreeMap::from([(
                    "ClientDone",
                    choice(vec![
                        then_seq(vec![send("ToMux", "WantNext")], "Idle"),
                        then_seq(vec![send("ToMux", "WantNext")], "Done"),
                    ]),
                )])),
                cfg: cfg_responder(),
                check: |e| matches!(e, ProjectError::Nondeterministic { state: StateId::Named("Idle"), .. }),
            },
            Case {
                name: "NondeterministicLocalSend",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    choice(vec![
                        then_seq(vec![call("ToResponder", "RequestRange")], "Busy"),
                        then_seq(vec![call("ToResponder", "RequestRange")], "Done"),
                    ]),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::Nondeterministic { state: StateId::Named("Idle"), .. }),
            },
            Case {
                name: "AmbiguousRepeatSkipsRepeatInSuffix",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    seq(
                        vec![
                            repeat(vec![call("ToResponder", "RequestRange")]),
                            repeat(vec![call("ToResponder", "ClientDone")]),
                            call("ToResponder", "RequestRange"),
                        ],
                        "Busy",
                    ),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::AmbiguousRepeat { origin: StateId::Named("Idle") }),
            },
            Case {
                name: "MixedAgency",
                graph: idle_graph(BTreeMap::from([
                    ("Fetch", seq(vec![call("ToResponder", "RequestRange")], "Busy")),
                    ("StartBatch", seq(vec![send("ToMux", "WantNext")], "Streaming")),
                ])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::MixedAgency { state: StateId::Named("Idle") }),
            },
            Case {
                name: "OccupancyDisagree",
                graph: {
                    let mut g = initiator_graph();
                    g.occupancy.insert("Idle", Occupancy::Remote);
                    g
                },
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::OccupancyDisagree { state: "Idle" }),
            },
            Case {
                name: "EmptyWireAlternatives",
                graph: idle_graph(BTreeMap::from([("ClientDone", RemainderAst { alternatives: vec![] })])),
                cfg: cfg_responder(),
                check: |e| matches!(e, ProjectError::EmptyWireAlternatives { state: "Idle", input: "ClientDone" }),
            },
            Case {
                name: "OverlappingInput",
                graph: idle_graph(BTreeMap::from([("Fetch", seq(vec![call("ToResponder", "RequestRange")], "Busy"))])),
                cfg: {
                    let mut cfg = cfg_initiator();
                    cfg.wire_inputs.insert("Fetch");
                    cfg
                },
                check: |e| matches!(e, ProjectError::OverlappingInput { input: "Fetch" }),
            },
            Case {
                name: "PeerSendAny",
                graph: idle_graph(BTreeMap::from([("Fetch", seq(vec![send_any("ToResponder")], "Busy"))])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::PeerSendAny { role: "ToResponder" }),
            },
            Case {
                name: "UnknownPeerPayload",
                graph: idle_graph(BTreeMap::from([("Fetch", seq(vec![call("ToResponder", "NotAPayload")], "Busy"))])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::UnknownPeerPayload { payload: "NotAPayload" }),
            },
            Case {
                name: "UnknownRole",
                graph: idle_graph(BTreeMap::from([("Fetch", seq(vec![call("ToNobody", "RequestRange")], "Busy"))])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::UnknownRole { role: "ToNobody" }),
            },
            Case {
                name: "UnknownInput",
                graph: idle_graph(BTreeMap::from([("NotListed", seq(vec![send("ToMux", "WantNext")], "Idle"))])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::UnknownInput { state: "Idle", input: "NotListed" }),
            },
        ];

        for case in cases {
            let err = project(&case.graph, &case.cfg).expect_err(case.name);
            assert!((case.check)(&err), "{}: unexpected {err:?}", case.name);
        }
    }

    #[test]
    fn unreachable_named_transitions_do_not_keep_dropped_synthetics() {
        let graph = TypeGraph {
            states: BTreeSet::from(["Idle", "Extra", "Done"]),
            initial: "Idle",
            occupancy: BTreeMap::new(),
            receives: BTreeMap::from([
                ("Idle", BTreeMap::from([("Pull", seq(vec![send("ToMux", "WantNext")], "Idle"))])),
                (
                    "Extra",
                    BTreeMap::from([(
                        "RequestRange",
                        seq(vec![call("ToInitiator", "StartBatch"), send("ToMux", "WantNext")], "Idle"),
                    )]),
                ),
            ]),
        };
        let got = project(&graph, &cfg_responder()).unwrap();
        assert!(got.states.contains(&named("Idle")));
        assert!(!got.states.contains(&named("Extra")));
        assert!(!got.states.contains(&named("Done")));
        assert!(got.states.iter().all(|s| matches!(s, StateId::Named(_))));
        assert!(!got.transitions.contains_key(&named("Extra")));
        assert!(got.transitions.values().all(|edges| edges.values().all(|to| got.states.contains(to))));
    }

    #[test]
    fn sim_open_dest_is_omitted_on_agency_holder() {
        let mut spec = SessionSpec::default();
        spec.init("Idle", "RequestRange", "Busy");
        spec.sim_open("Busy", "ClientDone", "OnlyOpen");
        spec.resp("Busy", "StartBatch", "Streaming");

        let holder = spec.project(Agency::Responder);
        assert!(!holder.states.contains(&named("OnlyOpen")));
        assert!(holder.states.contains(&named("Streaming")));

        let waiting = spec.project(Agency::Initiator);
        assert!(waiting.states.contains(&named("OnlyOpen")));
        assert_eq!(waiting.dest(&named("Busy"), "ClientDone"), named("OnlyOpen"));
    }

    #[test]
    fn driven_initiator_want_next_and_timeouts() {
        let g = initiator_graph();
        let cfg = cfg_initiator();
        let spec = table_37();
        check_want_next(&g, &cfg).unwrap();
        check_timeouts(&g, &cfg, &spec).unwrap();
        assert_eq!(spec.timeout("Busy"), Some(Duration::from_secs(60)));
        assert_eq!(spec.timeout("Streaming"), Some(Duration::from_secs(60)));
        assert_eq!(spec.timeout("Idle"), None);
        assert_eq!(spec.timeout("Done"), None);
    }

    #[test]
    fn undriven_responder_want_next_and_timeouts() {
        let g = responder_graph();
        let cfg = cfg_responder();
        check_want_next(&g, &cfg).unwrap();
        check_timeouts(&g, &cfg, &table_37()).unwrap();
    }

    #[test]
    fn driven_fetch_with_want_next_is_forbidden() {
        let mut g = initiator_graph();
        g.receives
            .get_mut("Idle")
            .unwrap()
            .insert("Fetch", seq(vec![call("ToResponder", "RequestRange"), send("ToMux", "WantNext")], "Busy"));
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::WantNextForbidden { state: "Idle", input: "Fetch" }), "{err:?}");
    }

    #[test]
    fn driven_fetch_with_set_timeout_is_forbidden() {
        let mut g = initiator_graph();
        g.receives
            .get_mut("Idle")
            .unwrap()
            .insert("Fetch", seq(vec![call("ToResponder", "RequestRange"), EffectAst::SetTimeout], "Busy"));
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::SetTimeoutForbidden { state: "Idle", input: "Fetch" }), "{err:?}");
    }

    #[test]
    fn driven_pull_without_set_timeout_when_busy_timed() {
        let mut g = initiator_graph();
        g.receives.get_mut("Busy").unwrap().insert("Pull", seq(vec![send("ToMux", "WantNext")], "Busy"));
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::SetTimeoutMissing { state: "Busy", input: "Pull" }), "{err:?}");
    }

    #[test]
    fn driven_no_blocks_without_clear_timeout() {
        let mut g = initiator_graph();
        g.receives
            .get_mut("Busy")
            .unwrap()
            .insert("NoBlocks", seq(vec![repeat(vec![send_any("ToCollector")])], "Idle"));
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::ClearTimeoutMissing { state: "Busy", input: "NoBlocks" }), "{err:?}");
    }

    #[test]
    fn driven_start_batch_without_want_next() {
        let mut g = initiator_graph();
        g.receives.get_mut("Busy").unwrap().insert("StartBatch", seq(vec![EffectAst::SetTimeout], "Streaming"));
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::WantNextMissing { state: "Busy", input: "StartBatch" }), "{err:?}");
    }

    #[test]
    fn driven_close_forbids_want_next() {
        let mut g = initiator_graph();
        g.receives.get_mut("Idle").unwrap().insert(
            "Close",
            RemainderAst {
                alternatives: vec![ThenAst {
                    parallel: vec![
                        vec![call("ToResponder", "ClientDone"), send("ToMux", "WantNext")],
                        vec![repeat(vec![send_any("ToCollector")])],
                    ],
                    next: "Done",
                }],
            },
        );
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::WantNextForbidden { state: "Idle", input: "Close" }), "{err:?}");
    }

    #[test]
    fn driven_fetch_requires_pull_on_remote_dest() {
        let mut g = initiator_graph();
        g.receives.get_mut("Busy").unwrap().remove("Pull");
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::MissingPull { dest: "Busy" }), "{err:?}");
    }

    #[test]
    fn want_next_in_star() {
        let graph = idle_graph(BTreeMap::from([("Pull", seq(vec![repeat(vec![send("ToMux", "WantNext")])], "Idle"))]));
        let err = check_want_next(&graph, &cfg_responder()).unwrap_err();
        assert!(matches!(err, WantNextError::WantNextInStar { state: "Idle", input: "Pull" }), "{err:?}");
    }

    #[test]
    fn want_next_must_be_send() {
        let graph = idle_graph(BTreeMap::from([("Pull", seq(vec![call("ToMux", "WantNext")], "Idle"))]));
        let err = check_want_next(&graph, &cfg_responder()).unwrap_err();
        assert!(matches!(err, WantNextError::WantNextMustBeSend { state: "Idle", input: "Pull" }), "{err:?}");
    }

    #[test]
    fn undriven_request_range_forbids_set_timeout() {
        let mut g = responder_graph();
        g.receives.get_mut("Idle").unwrap().insert(
            "RequestRange",
            choice(vec![
                then_seq(
                    vec![
                        call("ToInitiator", "StartBatch"),
                        repeat(vec![call("ToInitiator", "Block")]),
                        call("ToInitiator", "BatchDone"),
                        send("ToMux", "WantNext"),
                        EffectAst::SetTimeout,
                    ],
                    "Idle",
                ),
                then_seq(vec![call("ToInitiator", "NoBlocks"), send("ToMux", "WantNext")], "Idle"),
            ]),
        );
        let err = check_timeouts(&g, &cfg_responder(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::SetTimeoutForbidden { state: "Idle", input: "RequestRange" }), "{err:?}");
    }

    #[test]
    fn driven_missing_occupancy_is_error() {
        let mut g = initiator_graph();
        g.occupancy.remove("Busy");
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::MissingOccupancy { state: "Busy" }), "{err:?}");
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::MissingOccupancy { state: "Busy" }), "{err:?}");
    }

    #[test]
    fn peer_wire_send_must_be_call() {
        let mut g = initiator_graph();
        g.receives.get_mut("Idle").unwrap().insert("Fetch", seq(vec![send("ToResponder", "RequestRange")], "Busy"));
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(
            matches!(
                err,
                WantNextError::WirePayloadMustBeCall { state: "Idle", input: "Fetch", payload: "RequestRange" }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn driven_unlisted_occupancy_kind_is_error() {
        let mut g = initiator_graph();
        let mut cfg = cfg_initiator();
        cfg.local_inputs.insert("Pending");
        g.receives.get_mut("Busy").unwrap().insert("Pending", seq(vec![], "Busy"));
        let err = check_want_next(&g, &cfg).unwrap_err();
        assert!(matches!(err, WantNextError::UnlistedOccupancy { state: "Busy", input: "Pending" }), "{err:?}");
        let err = check_timeouts(&g, &cfg, &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::UnlistedOccupancy { state: "Busy", input: "Pending" }), "{err:?}");
    }

    #[test]
    fn unknown_input_is_error() {
        let mut g = initiator_graph();
        g.receives.get_mut("Busy").unwrap().insert("NotListed", seq(vec![send("ToMux", "WantNext")], "Busy"));
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::UnknownInput { state: "Busy", input: "NotListed" }), "{err:?}");
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::UnknownInput { state: "Busy", input: "NotListed" }), "{err:?}");

        let mut g = responder_graph();
        g.receives.get_mut("Idle").unwrap().insert("NotListed", seq(vec![send("ToMux", "WantNext")], "Idle"));
        let err = check_want_next(&g, &cfg_responder()).unwrap_err();
        assert!(matches!(err, WantNextError::UnknownInput { state: "Idle", input: "NotListed" }), "{err:?}");
        let err = check_timeouts(&g, &cfg_responder(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::UnknownInput { state: "Idle", input: "NotListed" }), "{err:?}");
    }

    #[test]
    fn driven_pull_dest_must_be_self() {
        let mut g = initiator_graph();
        g.receives
            .get_mut("Busy")
            .unwrap()
            .insert("Pull", seq(vec![send("ToMux", "WantNext"), EffectAst::SetTimeout], "Streaming"));
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::DestNotSelf { state: "Busy", input: "Pull" }), "{err:?}");
    }

    #[test]
    fn set_timeout_inside_repeat_does_not_count() {
        let mut g = initiator_graph();
        g.receives
            .get_mut("Busy")
            .unwrap()
            .insert("Pull", seq(vec![send("ToMux", "WantNext"), repeat(vec![EffectAst::SetTimeout])], "Busy"));
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::SetTimeoutMissing { state: "Busy", input: "Pull" }), "{err:?}");
    }

    #[test]
    fn clear_timeout_inside_repeat_does_not_count() {
        let mut g = initiator_graph();
        g.receives
            .get_mut("Busy")
            .unwrap()
            .insert("NoBlocks", seq(vec![repeat(vec![EffectAst::ClearTimeout, send_any("ToCollector")])], "Idle"));
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::ClearTimeoutMissing { state: "Busy", input: "NoBlocks" }), "{err:?}");
    }
}
