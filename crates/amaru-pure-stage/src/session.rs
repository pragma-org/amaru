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
//! orients that table for one [`Agency`]. Handler [`project`] hides configured
//! plumbing roles, timers, and local roles, unfolding remainder sequences onto a [`Cfsm`].
//!
//! Typestate remainder *use* lives in [`crate::typestate`].

#![expect(clippy::panic, clippy::unwrap_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Formatter, Write as _},
    time::Duration,
};

use crate::typestate::{
    EffectAst, InputName, Occupancy, PayloadName, RemainderAst, RoleName, StateName, ThenAst, TypeGraph,
};

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

/// Anonymous vertex in a projected CFSM. Names are not part of the automaton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Vertex(u32);

impl Display for Vertex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
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
/// Vertex identities are arbitrary; comparison is structural (labels + agency).
/// State names are diagnostic only.
#[derive(Debug, Clone)]
pub struct Cfsm {
    pub states: BTreeSet<Vertex>,
    pub initial: Vertex,
    pub terminal: BTreeSet<Vertex>,
    /// Who may send. Omitted for [`terminal`](Self::terminal) states.
    pub agency: BTreeMap<Vertex, Agency>,
    pub transitions: BTreeMap<Vertex, BTreeMap<Label, Vertex>>,
    names: BTreeMap<Vertex, StateName>,
}

impl PartialEq for Cfsm {
    fn eq(&self, other: &Self) -> bool {
        self.states == other.states
            && self.initial == other.initial
            && self.terminal == other.terminal
            && self.agency == other.agency
            && self.transitions == other.transitions
    }
}

impl Eq for Cfsm {}

impl Cfsm {
    /// Panic unless the two machines are the same deterministic automaton.
    ///
    /// Edge labels, agency, and terminal-ness must match. State names are not
    /// compared. This is not language inclusion: an extra edge on either side
    /// fails, and two machines that differ only by a redundant state fail too.
    /// A state that has edges and is not reachable from the initial vertex
    /// fails as well; those edges are not dropped.
    #[track_caller]
    pub fn assert_structurally_eq(&self, other: &Cfsm) {
        if let Err(reason) = self.structural_eq(other) {
            panic!(
                "assert_structurally_eq mismatch: {reason}\nleft:\n{}\n\nright:\n{}",
                self.fmt_table(),
                other.fmt_table()
            );
        }
    }

    fn structural_eq(&self, other: &Cfsm) -> Result<(), String> {
        if let Some(reason) = self.unreachable_edges() {
            return Err(reason);
        }
        if let Some(reason) = other.unreachable_edges() {
            return Err(reason);
        }
        let a = self.reachable_fragment();
        let b = other.reachable_fragment();
        let mut fwd = BTreeMap::new();
        let mut rev = BTreeMap::new();
        let mut stack = vec![(a.initial, b.initial)];
        fwd.insert(a.initial, b.initial);
        rev.insert(b.initial, a.initial);
        while let Some((x, y)) = stack.pop() {
            let xn = a.fmt_vertex(x);
            let yn = b.fmt_vertex(y);
            if a.terminal.contains(&x) != b.terminal.contains(&y) {
                return Err(format!(
                    "{xn} terminal={} vs {yn} terminal={}",
                    a.terminal.contains(&x),
                    b.terminal.contains(&y)
                ));
            }
            if a.agency.get(&x) != b.agency.get(&y) {
                return Err(format!("{xn} agency {:?} vs {yn} agency {:?}", a.agency.get(&x), b.agency.get(&y)));
            }
            let empty = BTreeMap::new();
            let xe = a.transitions.get(&x).unwrap_or(&empty);
            let ye = b.transitions.get(&y).unwrap_or(&empty);
            let xk: BTreeSet<_> = xe.keys().collect();
            let yk: BTreeSet<_> = ye.keys().collect();
            if xk != yk {
                return Err(format!("{xn} labels {xk:?} vs {yn} labels {yk:?}"));
            }
            for (lab, &xto) in xe {
                let yto = ye[lab];
                match (fwd.get(&xto), rev.get(&yto)) {
                    (None, None) => {
                        fwd.insert(xto, yto);
                        rev.insert(yto, xto);
                        stack.push((xto, yto));
                    }
                    (Some(&y2), Some(&x2)) if y2 == yto && x2 == xto => {}
                    _ => {
                        return Err(format!(
                            "{xn} --{lab}--> {} disagrees with {yn} --{lab}--> {}",
                            a.fmt_vertex(xto),
                            b.fmt_vertex(yto)
                        ));
                    }
                }
            }
        }
        if fwd.len() != a.states.len() || rev.len() != b.states.len() {
            return Err(format!("visited {}/{} vs {}/{}", fwd.len(), a.states.len(), rev.len(), b.states.len()));
        }
        Ok(())
    }

    /// Swap `Send`/`Recv` only. **Copy** `agency` (who-sends) and `terminal`.
    ///
    /// `dual(project(I))` equals `project(R)` for exclusive-agency specs without
    /// [`SessionSpec::sim_open`]. Handshake-shaped specs omit the `sim_open` edge
    /// on the agency holder, so the two projections are not duals; do not
    /// [`assert_structurally_eq`](Self::assert_structurally_eq) them.
    #[must_use]
    pub fn dual(&self) -> Cfsm {
        let transitions = self
            .transitions
            .iter()
            .map(|(from, edges)| {
                let swapped = edges
                    .iter()
                    .map(|(label, to)| (Label { direction: label.direction.opposite(), message: label.message }, *to))
                    .collect();
                (*from, swapped)
            })
            .collect();
        Cfsm {
            states: self.states.clone(),
            initial: self.initial,
            terminal: self.terminal.clone(),
            agency: self.agency.clone(),
            transitions,
            names: self.names.clone(),
        }
    }

    /// Destination of the unique edge from `from` whose message is `msg`.
    #[track_caller]
    pub fn dest(&self, from: Vertex, msg: &str) -> Vertex {
        let Some(edges) = self.transitions.get(&from) else {
            panic!("dest: no transitions from {}", self.fmt_vertex(from));
        };
        let mut found = None;
        for (label, to) in edges {
            if label.message == msg {
                if found.is_some() {
                    panic!("dest: multiple edges from {} with message {msg:?}", self.fmt_vertex(from));
                }
                found = Some(*to);
            }
        }
        found.unwrap_or_else(|| panic!("dest: no edge from {} with message {msg:?}", self.fmt_vertex(from)))
    }

    fn reachable(&self) -> BTreeSet<Vertex> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![self.initial];
        while let Some(s) = stack.pop() {
            if !seen.insert(s) {
                continue;
            }
            if let Some(edges) = self.transitions.get(&s) {
                stack.extend(edges.values().copied());
            }
        }
        seen
    }

    /// `None` when every vertex that has an edge is reachable from [`Self::initial`].
    ///
    /// Edgeless vertices are ignored here. Callers that compare machines must
    /// not treat an unreachable edge as absent: that would make a wrong
    /// component compare equal.
    fn unreachable_edges(&self) -> Option<String> {
        let reach = self.reachable();
        let bad: Vec<String> = self
            .transitions
            .iter()
            .filter(|(v, edges)| !edges.is_empty() && !reach.contains(v))
            .map(|(v, _)| self.fmt_vertex(*v))
            .collect();
        if bad.is_empty() { None } else { Some(format!("unreachable edges at {bad:?}")) }
    }

    fn reachable_fragment(&self) -> Cfsm {
        let reach = self.reachable();
        let transitions =
            self.transitions.iter().filter(|(s, _)| reach.contains(s)).map(|(s, e)| (*s, e.clone())).collect();
        let agency = self.agency.iter().filter(|(s, _)| reach.contains(s)).map(|(s, r)| (*s, *r)).collect();
        let names = self.names.iter().filter(|(s, _)| reach.contains(s)).map(|(s, n)| (*s, *n)).collect();
        let mut out =
            Cfsm { states: reach, initial: self.initial, terminal: BTreeSet::new(), agency, transitions, names };
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

    fn fmt_vertex(&self, v: Vertex) -> String {
        self.names.get(&v).map(|n| (*n).to_string()).unwrap_or_else(|| v.to_string())
    }

    fn fmt_table(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "initial: {}", self.fmt_vertex(self.initial));
        for state in &self.states {
            let agency = self.agency.get(state);
            let terminal = self.terminal.contains(state);
            let _ = writeln!(s, "{} agency={agency:?} terminal={terminal}", self.fmt_vertex(*state));
            if let Some(edges) = self.transitions.get(state) {
                for (label, to) in edges {
                    let _ = writeln!(s, "  {label} -> {}", self.fmt_vertex(*to));
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

    /// Panic on mismatch. Equality of the undirected table after `map`, not
    /// language inclusion. Does **not** compare timeouts or start state (`initial`).
    #[track_caller]
    pub fn assert_refines(&self, spec: &SessionSpec, map: impl Fn(&StateName) -> StateName) {
        let simplified = collapse_undirected(&self.transitions, map);
        assert_eq!(simplified, spec.transitions);
    }

    /// Orient: from a state where `role == agency`, outgoing labels are Send; otherwise Recv.
    /// `sim_open` edges are Recv of the aliased message for the waiting role.
    ///
    /// Panics if a state mentioned in the table is not reachable from the start,
    /// or if this orientation would leave a state that still has outgoing edges
    /// unreachable (a `sim_open` edge is the only way in, and this role omits it).
    /// Those edges would otherwise disappear from the comparison.
    pub fn project(&self, role: Agency) -> Cfsm {
        let Some(initial) = self.initial else {
            panic!("SessionSpec::project on empty spec");
        };
        self.panic_if_undirected_unreachable(initial);
        let mut alloc = Alloc::default();
        let initial_v = alloc.named(initial);

        let mut states = BTreeSet::from([initial_v]);
        let mut transitions: BTreeMap<Vertex, BTreeMap<Label, Vertex>> = BTreeMap::new();
        let mut agency = BTreeMap::new();

        for (from, per) in &self.transitions {
            let from_id = alloc.named(from);
            states.insert(from_id);
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
                let to_id = alloc.named(edge.to);
                states.insert(to_id);
                edges.insert(Label { direction, message: msg }, to_id);
            }
            if !edges.is_empty() {
                transitions.insert(from_id, edges);
                agency.insert(from_id, per.agency);
            }
        }

        let mut cfsm =
            Cfsm { states, initial: initial_v, terminal: BTreeSet::new(), agency, transitions, names: alloc.names };
        if let Some(reason) = cfsm.unreachable_edges() {
            panic!("SessionSpec::project: {reason} (outgoing edges not reachable from the start)");
        }
        cfsm.recompute_terminal();
        cfsm
    }

    /// Every state that appears as a source or a target can be reached from `initial`
    /// by following edges in either direction. `sim_open` counts: the undirected
    /// table is the diagram, before a role omits that edge.
    fn panic_if_undirected_unreachable(&self, initial: StateName) {
        let mut seen = BTreeSet::new();
        let mut stack = vec![initial];
        while let Some(state) = stack.pop() {
            if !seen.insert(state) {
                continue;
            }
            if let Some(per) = self.transitions.get(state) {
                stack.extend(per.transitions.values().map(|edge| edge.to));
            }
        }
        let mut mentioned = BTreeSet::new();
        for (from, per) in &self.transitions {
            mentioned.insert(*from);
            mentioned.extend(per.transitions.values().map(|edge| edge.to));
        }
        let unreachable: Vec<StateName> = mentioned.difference(&seen).copied().collect();
        if !unreachable.is_empty() {
            panic!("SessionSpec states not reachable from {initial}: {unreachable:?}");
        }
    }

    /// The mermaid notes are the agency of each state that has edges.
    ///
    /// A second note for the same state is rejected. A note that disagrees with
    /// the edges is rejected, including a `sim_open` state whose note does not
    /// say `Responder`. A state with edges and no note is rejected.
    pub fn assert_agency_notes(&self, notes: &[(&'static str, Agency)]) {
        let mut seen = BTreeSet::new();
        for &(state, noted) in notes {
            if !seen.insert(state) {
                panic!("session_spec!: duplicate note on {state}");
            }
            if let Some(per) = self.transitions.get(state)
                && per.agency != noted
            {
                panic!("session_spec!: note on {state} says {noted:?} but the edges are {:?}", per.agency);
            }
        }
        for state in self.transitions.keys() {
            if !seen.contains(state) {
                panic!("session_spec!: missing `note left of {state}: Initiator` or `Responder`");
            }
        }
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

/// Hand-written per protocol. `driven` selects occupancy-based timeout tables.
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
    ParallelWire {
        state: StateName,
        input: InputName,
    },
    NoWireFromLocal {
        state: StateName,
        input: InputName,
    },
    MixedHidableWireChoice {
        state: StateName,
        input: InputName,
    },
    EmptyWireAlternatives {
        state: StateName,
        input: InputName,
    },
    OverlappingInput {
        input: InputName,
    },
    Nondeterministic {
        state: Vertex,
        label: String,
    },
    MixedAgency {
        state: Vertex,
    },
    AmbiguousRepeat {
        origin: Vertex,
    },
    RepeatStarTooWide {
        origin: Vertex,
    },
    SequencedRepeat {
        origin: Vertex,
    },
    TrailingRepeat {
        origin: Vertex,
    },
    /// A star and a later send both stay on the same vertex. Two self-loops are
    /// a larger language than “the star, then that send”.
    RepeatExitStays {
        origin: Vertex,
    },
    /// A plumbing arm contains a peer wire effect. Dropping the arm would hide it.
    PlumbingHasWire {
        state: StateName,
        input: InputName,
    },
    /// A plumbing arm finishes in a different state. That is a silent transition.
    PlumbingChangesState {
        state: StateName,
        input: InputName,
    },
    /// `state` has wire edges and cannot be reached from the graph’s initial state.
    Unreachable {
        state: StateName,
    },
    PeerSendAny {
        role: RoleName,
    },
    UnknownPeerPayload {
        payload: PayloadName,
    },
    UnknownRole {
        role: RoleName,
    },
    OccupancyDisagree {
        state: StateName,
    },
    UnknownInput {
        state: StateName,
        input: InputName,
    },
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
            ProjectError::SequencedRepeat { origin } => {
                write!(f, "second wire repeat on {origin}")
            }
            ProjectError::TrailingRepeat { origin } => {
                write!(f, "trailing wire repeat at {origin} moves to a different state")
            }
            ProjectError::RepeatExitStays { origin } => {
                write!(f, "repeat at {origin} is followed by a send that stays in the same state")
            }
            ProjectError::PlumbingHasWire { state, input } => {
                write!(f, "plumbing input {input} at {state} has a peer wire effect")
            }
            ProjectError::PlumbingChangesState { state, input } => {
                write!(f, "plumbing input {input} at {state} finishes in a different state")
            }
            ProjectError::Unreachable { state } => {
                write!(f, "state {state} has wire edges but is not reachable from the initial state")
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

/// Project a remainder graph onto the peer channel.
///
/// Plumbing inputs are not wire events. An arm that still contains a peer
/// message, or that finishes in another state, is an error: dropping the arm
/// would hide that edge. A wire edge in a state that cannot be reached from
/// the initial state is an error rather than something the comparison skips.
pub fn project(graph: &TypeGraph, cfg: &ProjectionConfig) -> Result<Cfsm, ProjectError> {
    if let Some(input) = overlapping_input(cfg) {
        return Err(ProjectError::OverlappingInput { input });
    }

    let mut proj = Projector { cfg, alloc: Alloc::default(), states: BTreeSet::new(), transitions: BTreeMap::new() };

    for name in &graph.states {
        let v = proj.alloc.named(name);
        proj.states.insert(v);
    }

    for (state, inputs) in &graph.receives {
        for (input, rem) in inputs {
            if cfg.plumbing_inputs.contains(input) {
                proj.reject_plumbing(state, input, rem)?;
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
                let origin = proj.alloc.named(state);
                for (seq, next) in wire_bearing {
                    proj.expand_seq(origin, &seq, next)?;
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
                            state: proj.alloc.named(state),
                            label: Label::recv(m).to_string(),
                        });
                    }
                    let next = *nexts.iter().next().unwrap();
                    let from = proj.alloc.named(state);
                    let to = proj.alloc.named(next);
                    proj.emit(from, Label::recv(m), to)?;
                } else {
                    let from = proj.alloc.named(state);
                    let dest = proj.alloc.fresh();
                    proj.emit(from, Label::recv(m), dest)?;
                    for (seq, next) in wire_bearing {
                        proj.expand_seq(dest, &seq, next)?;
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
            (true, true) => {
                return Err(ProjectError::MixedAgency { state: *state });
            }
            (true, false) => cfg.role,
            (false, true) => cfg.role.opposite(),
            (false, false) => continue,
        };
        agency.insert(*state, inferred);
    }

    if !graph.occupancy.is_empty() {
        for (name, occ) in &graph.occupancy {
            let id = proj.alloc.named(name);
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

    let initial = proj.alloc.named(graph.initial);
    let mut cfsm = Cfsm {
        states: proj.states,
        initial,
        terminal: BTreeSet::new(),
        agency,
        transitions: proj.transitions,
        names: proj.alloc.names,
    };
    cfsm.states.insert(initial);
    if let Some(state) = unreachable_named_state(&cfsm) {
        return Err(ProjectError::Unreachable { state });
    }
    if let Some(reason) = cfsm.unreachable_edges() {
        panic!("project: {reason}");
    }
    drop_unreachable(&mut cfsm);
    cfsm.recompute_terminal();
    Ok(cfsm)
}

/// First named state that has an edge and is not reachable from the initial vertex.
///
/// Synthetic vertices are skipped so the named source, which is allocated
/// first, is the one reported. A synthetic with no named source is left for
/// [`Cfsm::unreachable_edges`].
fn unreachable_named_state(cfsm: &Cfsm) -> Option<StateName> {
    let reach = cfsm.reachable();
    cfsm.transitions.iter().find_map(|(vertex, edges)| {
        if edges.is_empty() || reach.contains(vertex) { None } else { cfsm.names.get(vertex).copied() }
    })
}

/// Project `graph` with `cfg` and check it against `spec`.
///
/// Runs timeout well-formedness, projection, structural equality with
/// `spec.project(cfg.role)`, and wire-input coverage.
#[track_caller]
pub fn assert_projects(graph: &TypeGraph, cfg: &ProjectionConfig, spec: &SessionSpec) -> Cfsm {
    check_timeouts(graph, cfg, spec).unwrap_or_else(|e| panic!("{e}"));
    let projected = project(graph, cfg).unwrap_or_else(|e| panic!("{e}"));
    projected.assert_structurally_eq(&spec.project(cfg.role));
    assert_wire_inputs_cover_receives(graph, cfg, spec);
    projected
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

#[derive(Default)]
struct Alloc {
    next: u32,
    named: BTreeMap<StateName, Vertex>,
    names: BTreeMap<Vertex, StateName>,
}

impl Alloc {
    fn named(&mut self, name: StateName) -> Vertex {
        if let Some(&v) = self.named.get(name) {
            return v;
        }
        let v = self.fresh();
        self.named.insert(name, v);
        self.names.insert(v, name);
        v
    }

    fn fresh(&mut self) -> Vertex {
        let v = Vertex(self.next);
        self.next += 1;
        v
    }
}

struct Projector<'a> {
    cfg: &'a ProjectionConfig,
    alloc: Alloc,
    states: BTreeSet<Vertex>,
    transitions: BTreeMap<Vertex, BTreeMap<Label, Vertex>>,
}

impl Projector<'_> {
    /// Plumbing is invisible on the peer channel, so the arm must not change
    /// that channel: no remaining peer effect, and the next state is this state.
    fn reject_plumbing(&self, state: StateName, input: InputName, rem: &RemainderAst) -> Result<(), ProjectError> {
        for alt in &rem.alternatives {
            let seq = self.hide_parallel(state, input, alt)?;
            if !seq.is_empty() {
                return Err(ProjectError::PlumbingHasWire { state, input });
            }
            if alt.next != state {
                return Err(ProjectError::PlumbingChangesState { state, input });
            }
        }
        Ok(())
    }

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
            | EffectAst::Detach { .. }
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
    ///
    /// One wire `Repeat` is a self-loop on the vertex it is expanded from. The
    /// exit is a later wire label that leaves that vertex, or the successor
    /// already being that vertex when the star is the whole sequence.
    /// Another wire `Repeat` on that same vertex, a trailing star whose
    /// successor is a different vertex, or a following send that stays on the
    /// star's vertex, has no edge in this machine. Two self-loops would be
    /// either message at any time, which is larger than “the star, then the send”.
    fn expand_seq(&mut self, origin: Vertex, seq: &[&EffectAst], named_next: StateName) -> Result<(), ProjectError> {
        assert!(!seq.is_empty(), "expand_seq on hide-filtered empty seq at {origin}");
        let mut state = origin;
        let mut repeated_at = None;
        for (i, e) in seq.iter().copied().enumerate() {
            match e {
                EffectAst::Repeat(body) => {
                    let body = self.non_hidable_flat(body)?;
                    assert!(!body.is_empty(), "expand_seq Repeat body empty after hide at {state}");
                    let Some(m) = self.single_wire_send(&body) else {
                        return Err(ProjectError::RepeatStarTooWide { origin: state });
                    };
                    if repeated_at == Some(state) {
                        return Err(ProjectError::SequencedRepeat { origin: state });
                    }
                    let exit = self.first_wire(&seq[i + 1..]);
                    if exit.is_some_and(|head| head == m) {
                        return Err(ProjectError::AmbiguousRepeat { origin: state });
                    }
                    if exit.is_none() && self.alloc.named(named_next) != state {
                        return Err(ProjectError::TrailingRepeat { origin: state });
                    }
                    self.emit(state, Label::send(m), state)?;
                    repeated_at = Some(state);
                }
                EffectAst::Call { payload, .. } | EffectAst::Send { payload, .. } => {
                    if !self.cfg.wire_payload.contains(payload) {
                        panic!("expand_seq: payload {payload} missing from wire_payload at {state}");
                    }
                    let m = *payload;
                    if i + 1 == seq.len() {
                        let to = self.alloc.named(named_next);
                        if repeated_at == Some(state) && to == state {
                            return Err(ProjectError::RepeatExitStays { origin: state });
                        }
                        self.emit(state, Label::send(m), to)?;
                        return Ok(());
                    }
                    let dest = self.alloc.fresh();
                    self.emit(state, Label::send(m), dest)?;
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
                | EffectAst::Detach { .. }
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
            | EffectAst::Detach { .. }
            | EffectAst::AddStage => None,
        }
    }

    fn first_wire<'a>(&'a self, suffix: &'a [&EffectAst]) -> Option<&'static str> {
        suffix.iter().copied().find_map(|e| self.as_wire_send(e))
    }

    fn emit(&mut self, from: Vertex, label: Label, to: Vertex) -> Result<(), ProjectError> {
        self.states.insert(from);
        self.states.insert(to);
        let edges = self.transitions.entry(from).or_default();
        if let Some(existing) = edges.get(&label)
            && existing != &to
        {
            return Err(ProjectError::Nondeterministic { state: from, label: label.to_string() });
        }
        edges.insert(label, to);
        Ok(())
    }
}

fn drop_unreachable(cfsm: &mut Cfsm) {
    cfsm.states.insert(cfsm.initial);
    let reach = cfsm.reachable();
    cfsm.states.retain(|s| reach.contains(s));
    cfsm.transitions.retain(|s, _| reach.contains(s));
    for edges in cfsm.transitions.values_mut() {
        edges.retain(|_, to| cfsm.states.contains(to));
    }
    cfsm.agency.retain(|s, _| cfsm.states.contains(s));
    cfsm.names.retain(|s, _| cfsm.states.contains(s));
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

/// Why [`check_timeouts`] rejected a remainder graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeoutError {
    SetTimeoutForbidden {
        state: StateName,
        input: InputName,
    },
    SetTimeoutMissing {
        state: StateName,
        input: InputName,
    },
    ClearTimeoutMissing {
        state: StateName,
        input: InputName,
    },
    /// A local input leaves the switch for a timed remote state that has no
    /// plumbing arm. That entry is not allowed to arm the timer, and `drive`
    /// injects `Pull` on the destination, so the timer would never be required.
    TimedStateWithoutPull {
        state: StateName,
    },
    MissingOccupancy {
        state: StateName,
    },
    UnlistedOccupancy {
        state: StateName,
        input: InputName,
    },
    UnknownInput {
        state: StateName,
        input: InputName,
    },
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
            TimeoutError::TimedStateWithoutPull { state } => {
                write!(f, "timed remote state {state} has no plumbing arm to arm the timer")
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

    // A local arm from the switch into a timed remote state must not itself arm
    // the timer (`SetTimeout` is forbidden there). `drive` injects `Pull` on
    // that destination. If the arm is missing, nothing is required to call
    // `SetTimeout`. Later remote states arm on their own wire arms.
    if cfg.driven {
        for (state, inputs) in &graph.receives {
            if occupancy_of(graph, state) != Some(Occupancy::Switch) {
                continue;
            }
            for (input, rem) in inputs {
                if !matches!(input_kind(cfg, input), InputKind::Local) {
                    continue;
                }
                for alt in &rem.alternatives {
                    if occupancy_of(graph, alt.next) == Some(Occupancy::Remote)
                        && spec.timeout(alt.next).is_some()
                        && !has_plumbing_arm(graph, cfg, alt.next)
                    {
                        return Err(TimeoutError::TimedStateWithoutPull { state: alt.next });
                    }
                }
            }
        }
    }
    Ok(())
}

fn has_plumbing_arm(graph: &TypeGraph, cfg: &ProjectionConfig, state: StateName) -> bool {
    graph.receives.get(state).is_some_and(|inputs| inputs.keys().any(|input| cfg.plumbing_inputs.contains(input)))
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
            | EffectAst::Detach { .. }
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
        | EffectAst::Detach { .. }
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
        got.assert_structurally_eq(&spec);
        let idle = got.initial;
        assert_eq!(got.agency.get(&idle), Some(&Agency::Initiator));
        let busy = got.dest(idle, "RequestRange");
        assert_eq!(got.agency.get(&busy), Some(&Agency::Responder));
        let streaming = got.dest(busy, "StartBatch");
        assert_eq!(got.agency.get(&streaming), Some(&Agency::Responder));
        let done = got.dest(idle, "ClientDone");
        assert!(got.terminal.contains(&done));
        assert_eq!(got.agency.get(&done), None);
        assert_eq!(got.dest(busy, "NoBlocks"), idle);
        assert_eq!(got.dest(streaming, "Block"), streaming);
        assert_eq!(got.dest(streaming, "BatchDone"), idle);
    }

    #[test]
    fn responder_request_range_synthetics_refine_spec() {
        let spec = table_37().project(Agency::Responder);
        let got = project(&responder_graph(), &cfg_responder()).unwrap();
        let idle = got.initial;
        let req = got.dest(idle, "RequestRange");
        let start = got.dest(req, "StartBatch");
        assert_eq!(got.dest(req, "NoBlocks"), idle);
        assert_eq!(got.dest(start, "Block"), start);
        assert_eq!(got.dest(start, "BatchDone"), idle);
        assert!(got.terminal.contains(&got.dest(idle, "ClientDone")));
        got.assert_structurally_eq(&spec);
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
        let propose_i = spec_i.initial;
        let confirm_i = spec_i.dest(propose_i, "Propose");
        let done_i = spec_i.dest(confirm_i, "Propose");
        assert_eq!(spec_i.dest(confirm_i, "Accept"), done_i);
        assert!(spec_i.transitions.get(&confirm_i).unwrap().keys().all(|l| l.direction == Direction::Recv));
        assert_eq!(spec_i.agency.get(&confirm_i), Some(&Agency::Responder));

        let spec_r = spec.project(Agency::Responder);
        let propose_r = spec_r.initial;
        let confirm_r = spec_r.dest(propose_r, "Propose");
        let done_r = spec_r.dest(confirm_r, "Accept");
        assert_eq!(spec_r.dest(confirm_r, "Refuse"), done_r);
        assert_eq!(spec_r.dest(confirm_r, "QueryReply"), done_r);
        assert!(spec_r.transitions.get(&confirm_r).unwrap().keys().all(|l| l.direction == Direction::Send));
        assert!(spec_r.transitions.get(&confirm_r).unwrap().keys().all(|l| l.message != "Propose"));
        assert_eq!(spec_r.agency.get(&confirm_r), Some(&Agency::Responder));
        // Handshake is not dual(project(I)) == project(R): sim_open is Recv for
        // the waiting role and omitted for the agency holder.
    }

    #[test]
    fn spec_projection_table_uses_session_spec_names() {
        let dump = table_37().project(Agency::Initiator).fmt_table();
        assert!(dump.contains("Idle"), "{dump}");
        assert!(dump.contains("Busy"), "{dump}");
        assert!(dump.contains("Streaming"), "{dump}");
        assert!(!dump.contains("v0"), "{dump}");
        assert_eq!(
            dump,
            "initial: Idle
Idle agency=Some(Initiator) terminal=false
  !ClientDone -> Done
  !RequestRange -> Busy
Busy agency=Some(Responder) terminal=false
  ?NoBlocks -> Idle
  ?StartBatch -> Streaming
Streaming agency=Some(Responder) terminal=false
  ?BatchDone -> Idle
  ?Block -> Streaming
Done agency=None terminal=true
"
        );
    }

    #[test]
    fn dual_of_spec_initiator_equals_spec_responder() {
        // Table 3.7 has exclusive agency and no sim_open; duality holds only then.
        let spec = table_37();
        let spec_i = spec.project(Agency::Initiator);
        let spec_r = spec.project(Agency::Responder);
        assert_eq!(spec_i.dual(), spec_r);
        assert_eq!(spec_r.dual(), spec_i);
    }

    #[test]
    fn assert_refines_ignores_timeouts() {
        let mut timed = table_37();
        timed.set_timeout("Idle", Duration::from_secs(1));
        timed.project(Agency::Initiator).assert_structurally_eq(&table_37().project(Agency::Initiator));
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
        let syn = got.dest(got.initial, "RequestRange");
        assert_eq!(got.dest(syn, "Block"), syn);
        assert_eq!(got.dest(syn, "BatchDone"), got.initial);
    }

    #[test]
    fn trailing_repeat_stays_in_the_current_state() {
        let graph = idle_graph(BTreeMap::from([(
            "Fetch",
            seq(vec![repeat(vec![call("ToResponder", "RequestRange")])], "Idle"),
        )]));
        let got = project(&graph, &cfg_initiator()).unwrap();
        assert_eq!(got.states.len(), 1);
        assert_eq!(got.dest(got.initial, "RequestRange"), got.initial);
    }

    #[test]
    fn one_repeat_between_distinct_sends() {
        let graph = idle_graph(BTreeMap::from([(
            "Fetch",
            seq(
                vec![
                    call("ToResponder", "RequestRange"),
                    repeat(vec![call("ToResponder", "ClientDone")]),
                    call("ToResponder", "StartBatch"),
                ],
                "Done",
            ),
        )]));
        let got = project(&graph, &cfg_initiator()).unwrap();
        let mid = got.dest(got.initial, "RequestRange");
        assert_ne!(mid, got.initial);
        assert_eq!(got.dest(mid, "ClientDone"), mid);
        let done = got.dest(mid, "StartBatch");
        assert_ne!(done, mid);
        assert!(got.terminal.contains(&done));
    }

    #[test]
    fn repeats_separated_by_a_send_are_independent() {
        let graph = idle_graph(BTreeMap::from([(
            "Fetch",
            seq(
                vec![
                    repeat(vec![call("ToResponder", "RequestRange")]),
                    call("ToResponder", "ClientDone"),
                    repeat(vec![call("ToResponder", "StartBatch")]),
                    call("ToResponder", "BatchDone"),
                ],
                "Done",
            ),
        )]));
        let got = project(&graph, &cfg_initiator()).unwrap();
        let idle = got.initial;
        assert_eq!(got.dest(idle, "RequestRange"), idle);
        let mid = got.dest(idle, "ClientDone");
        assert_ne!(mid, idle);
        assert_eq!(got.dest(mid, "StartBatch"), mid);
        assert!(got.terminal.contains(&got.dest(mid, "BatchDone")));
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
                check: |e| matches!(e, ProjectError::Nondeterministic { .. }),
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
                check: |e| matches!(e, ProjectError::Nondeterministic { .. }),
            },
            Case {
                name: "SequencedRepeat",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    seq(
                        vec![
                            repeat(vec![call("ToResponder", "RequestRange")]),
                            repeat(vec![call("ToResponder", "ClientDone")]),
                        ],
                        "Idle",
                    ),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::SequencedRepeat { .. }),
            },
            Case {
                name: "SequencedRepeatThenSend",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    seq(
                        vec![
                            repeat(vec![call("ToResponder", "RequestRange")]),
                            repeat(vec![call("ToResponder", "ClientDone")]),
                            call("ToResponder", "StartBatch"),
                        ],
                        "Done",
                    ),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::SequencedRepeat { .. }),
            },
            Case {
                name: "TrailingRepeat",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    seq(vec![repeat(vec![call("ToResponder", "RequestRange")])], "Done"),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::TrailingRepeat { .. }),
            },
            Case {
                name: "TrailingRepeatAfterSend",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    seq(
                        vec![call("ToResponder", "RequestRange"), repeat(vec![call("ToResponder", "ClientDone")])],
                        "Idle",
                    ),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::TrailingRepeat { .. }),
            },
            Case {
                name: "TrailingRepeatOnWireInput",
                graph: idle_graph(BTreeMap::from([(
                    "RequestRange",
                    seq(vec![repeat(vec![call("ToInitiator", "Block")])], "Done"),
                )])),
                cfg: cfg_responder(),
                check: |e| matches!(e, ProjectError::TrailingRepeat { .. }),
            },
            Case {
                name: "MixedAgency",
                graph: idle_graph(BTreeMap::from([
                    ("Fetch", seq(vec![call("ToResponder", "RequestRange")], "Busy")),
                    ("StartBatch", seq(vec![send("ToMux", "WantNext")], "Streaming")),
                ])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::MixedAgency { .. }),
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
            Case {
                name: "PlumbingHasWire",
                graph: {
                    let mut g = initiator_graph();
                    g.receives.get_mut("Busy").unwrap().insert(
                        "Pull",
                        seq(
                            vec![send("ToMux", "WantNext"), call("ToResponder", "ClientDone"), EffectAst::SetTimeout],
                            "Busy",
                        ),
                    );
                    g
                },
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::PlumbingHasWire { state: "Busy", input: "Pull" }),
            },
            Case {
                name: "PlumbingChangesState",
                graph: {
                    let mut g = initiator_graph();
                    g.receives
                        .get_mut("Busy")
                        .unwrap()
                        .insert("Pull", seq(vec![send("ToMux", "WantNext"), EffectAst::SetTimeout], "Idle"));
                    g
                },
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::PlumbingChangesState { state: "Busy", input: "Pull" }),
            },
            Case {
                name: "RepeatExitStays",
                graph: idle_graph(BTreeMap::from([(
                    "Fetch",
                    seq(
                        vec![repeat(vec![call("ToResponder", "RequestRange")]), call("ToResponder", "ClientDone")],
                        "Idle",
                    ),
                )])),
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::RepeatExitStays { .. }),
            },
            Case {
                name: "Unreachable",
                graph: TypeGraph {
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
                },
                cfg: cfg_responder(),
                check: |e| matches!(e, ProjectError::Unreachable { state: "Extra" }),
            },
            Case {
                name: "OccupancyTerminalHasEdge",
                graph: {
                    let mut g = initiator_graph();
                    g.occupancy.insert("Busy", Occupancy::Terminal);
                    g
                },
                cfg: cfg_initiator(),
                check: |e| matches!(e, ProjectError::OccupancyDisagree { state: "Busy" }),
            },
            Case {
                name: "OverlappingPlumbing",
                graph: idle_graph(BTreeMap::from([("Fetch", seq(vec![call("ToResponder", "RequestRange")], "Busy"))])),
                cfg: {
                    let mut cfg = cfg_initiator();
                    cfg.plumbing_inputs.insert("Fetch");
                    cfg
                },
                check: |e| matches!(e, ProjectError::OverlappingInput { input: "Fetch" }),
            },
        ];

        for case in cases {
            let err = project(&case.graph, &case.cfg).expect_err(case.name);
            assert!((case.check)(&err), "{}: unexpected {err:?}", case.name);
        }
    }

    #[test]
    fn projected_edges_stay_inside_the_state_set() {
        let got = project(&initiator_graph(), &cfg_initiator()).unwrap();
        assert!(got.states.contains(&got.initial));
        assert!(got.transitions.keys().all(|from| got.states.contains(from)));
        assert!(got.transitions.values().all(|edges| edges.values().all(|to| got.states.contains(to))));
    }

    #[test]
    #[should_panic(expected = "not reachable from")]
    fn spec_orphan_state_is_rejected() {
        let mut spec = SessionSpec::default();
        spec.start("Idle");
        spec.init("Idle", "Hello", "Mid");
        spec.init("Orphan", "Hello", "Done");
        let _ = spec.project(Agency::Initiator);
    }

    #[test]
    #[should_panic(expected = "not reachable from")]
    fn spec_start_that_misses_the_diagram_is_rejected() {
        let mut spec = SessionSpec::default();
        spec.start("Done");
        spec.init("Idle", "Hello", "Mid");
        let _ = spec.project(Agency::Initiator);
    }

    #[test]
    fn sim_open_entry_is_visible_to_the_waiting_role() {
        let spec = sim_open_secret();
        let waiting = spec.project(Agency::Initiator);
        let mid = waiting.dest(waiting.initial, "Hello");
        let secret = waiting.dest(mid, "Hello");
        assert_ne!(secret, mid);
        let done = waiting.dest(secret, "Accept");
        assert!(waiting.terminal.contains(&done));
    }

    #[test]
    #[should_panic(expected = "outgoing edges not reachable")]
    fn sim_open_cannot_be_the_only_way_into_a_state_that_sends() {
        let _ = sim_open_secret().project(Agency::Responder);
    }

    fn sim_open_secret() -> SessionSpec {
        let mut spec = SessionSpec::default();
        spec.init("Idle", "Hello", "Mid");
        spec.sim_open("Mid", "Hello", "Secret");
        spec.resp("Secret", "Accept", "Done");
        spec
    }

    #[test]
    fn structural_eq_ignores_state_names() {
        let mut named = SessionSpec::default();
        named.init("Idle", "Ping", "Done");
        let mut renamed = SessionSpec::default();
        renamed.init("Start", "Ping", "End");
        named.project(Agency::Initiator).assert_structurally_eq(&renamed.project(Agency::Initiator));
    }

    #[test]
    #[should_panic(expected = "labels")]
    fn structural_eq_rejects_an_extra_edge() {
        let mut slim = SessionSpec::default();
        slim.init("Idle", "Ping", "Done");
        let mut extra = SessionSpec::default();
        extra.init("Idle", "Ping", "Done");
        extra.init("Idle", "Pong", "Done");
        slim.project(Agency::Initiator).assert_structurally_eq(&extra.project(Agency::Initiator));
    }

    #[test]
    #[should_panic(expected = "agency")]
    fn structural_eq_rejects_agency_mismatch() {
        let mut send = SessionSpec::default();
        send.init("Idle", "Ping", "Done");
        let mut recv = SessionSpec::default();
        recv.resp("Idle", "Ping", "Done");
        send.project(Agency::Initiator).assert_structurally_eq(&recv.project(Agency::Initiator));
    }

    #[test]
    #[should_panic(expected = "terminal=")]
    fn structural_eq_rejects_a_redundant_intermediate_state() {
        let mut direct = SessionSpec::default();
        direct.init("Idle", "Ping", "Done");
        let mut via = SessionSpec::default();
        via.init("Idle", "Ping", "Mid");
        via.init("Mid", "Pong", "Done");
        direct.project(Agency::Initiator).assert_structurally_eq(&via.project(Agency::Initiator));
    }

    #[test]
    #[should_panic(expected = "disagrees")]
    fn structural_eq_rejects_a_crossed_destination() {
        let mut left = SessionSpec::default();
        left.init("Idle", "A", "X");
        left.init("Idle", "B", "Y");
        left.init("X", "C", "Y");
        left.init("Y", "C", "X");
        let mut right = SessionSpec::default();
        right.init("Idle", "A", "X");
        right.init("Idle", "B", "X");
        right.init("X", "C", "X");
        left.project(Agency::Initiator).assert_structurally_eq(&right.project(Agency::Initiator));
    }

    #[test]
    fn sim_open_dest_is_omitted_on_agency_holder() {
        let mut spec = SessionSpec::default();
        spec.init("Idle", "RequestRange", "Busy");
        spec.sim_open("Busy", "ClientDone", "OnlyOpen");
        spec.resp("Busy", "StartBatch", "Streaming");

        assert_eq!(spec.initial, Some("Idle"));

        let holder = spec.project(Agency::Responder);
        let busy_h = holder.dest(holder.initial, "RequestRange");
        assert!(holder.transitions[&busy_h].keys().all(|l| l.message != "ClientDone"));
        let streaming = holder.dest(busy_h, "StartBatch");
        assert_ne!(streaming, busy_h);

        let waiting = spec.project(Agency::Initiator);
        let busy_w = waiting.dest(waiting.initial, "RequestRange");
        let only_open = waiting.dest(busy_w, "ClientDone");
        assert_ne!(only_open, busy_w);
        assert!(waiting.terminal.contains(&only_open) || waiting.transitions.contains_key(&only_open));
    }

    #[test]
    fn driven_initiator_timeouts() {
        let g = initiator_graph();
        let cfg = cfg_initiator();
        let spec = table_37();
        check_timeouts(&g, &cfg, &spec).unwrap();
        assert_eq!(spec.timeout("Busy"), Some(Duration::from_secs(60)));
        assert_eq!(spec.timeout("Streaming"), Some(Duration::from_secs(60)));
        assert_eq!(spec.timeout("Idle"), None);
        assert_eq!(spec.timeout("Done"), None);
    }

    #[test]
    fn undriven_responder_timeouts() {
        let g = responder_graph();
        let cfg = cfg_responder();
        check_timeouts(&g, &cfg, &table_37()).unwrap();
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
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::MissingOccupancy { state: "Busy" }), "{err:?}");
    }

    #[test]
    fn driven_unlisted_occupancy_kind_is_error() {
        let mut g = initiator_graph();
        let mut cfg = cfg_initiator();
        cfg.local_inputs.insert("Pending");
        g.receives.get_mut("Busy").unwrap().insert("Pending", seq(vec![], "Busy"));
        let err = check_timeouts(&g, &cfg, &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::UnlistedOccupancy { state: "Busy", input: "Pending" }), "{err:?}");
    }

    #[test]
    fn unknown_input_is_error() {
        let mut g = initiator_graph();
        g.receives.get_mut("Busy").unwrap().insert("NotListed", seq(vec![send("ToMux", "WantNext")], "Busy"));
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::UnknownInput { state: "Busy", input: "NotListed" }), "{err:?}");

        let mut g = responder_graph();
        g.receives.get_mut("Idle").unwrap().insert("NotListed", seq(vec![send("ToMux", "WantNext")], "Idle"));
        let err = check_timeouts(&g, &cfg_responder(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::UnknownInput { state: "Idle", input: "NotListed" }), "{err:?}");
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
    fn driven_timed_remote_without_pull_is_error() {
        let mut g = initiator_graph();
        g.receives.get_mut("Busy").unwrap().remove("Pull");
        let err = check_timeouts(&g, &cfg_initiator(), &table_37()).unwrap_err();
        assert!(matches!(err, TimeoutError::TimedStateWithoutPull { state: "Busy" }), "{err:?}");
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

#[cfg(test)]
mod unused_messages {
    use std::collections::BTreeSet;

    use crate::typestate::prelude::*;

    make_states!(Live { Start; Wait, End });
    on_receive!(Start as StartIn {});
    on_receive!(Wait as WaitIn {});
    on_receive!(End as EndIn {});

    define_messages! {
        #[derive(Debug, Clone, PartialEq, Eq)]
        enum Hs {
            Propose,
            Accept,
            QueryReply,
        }
    }

    #[test]
    fn unused_variant_is_not_an_edge() {
        let spec = crate::session_spec! {
            Hs unused [QueryReply];
            [*] --> Start
            Start --> Wait: Propose
            Wait --> End: Accept
            note left of Start: Initiator
            note left of Wait: Responder
        };
        let edges = spec.edge_labels().collect::<BTreeSet<_>>();
        assert_eq!(edges, BTreeSet::from(["Accept", "Propose"]));
    }

    #[test]
    #[should_panic(expected = "QueryReply is neither in the spec table nor listed unused")]
    fn omitted_unused_variant_is_rejected() {
        let _ = crate::session_spec! {
            Hs;
            [*] --> Start
            Start --> Wait: Propose
            Wait --> End: Accept
            note left of Start: Initiator
            note left of Wait: Responder
        };
    }

    #[test]
    #[should_panic(expected = "duplicate note on Wait")]
    fn duplicate_note_is_rejected() {
        let _ = crate::session_spec! {
            Hs unused [QueryReply];
            [*] --> Start
            Start --> Wait: Propose
            Wait --> End: Accept
            note left of Start: Initiator
            note left of Wait: Responder
            note right of Wait: Responder
        };
    }

    #[test]
    #[should_panic(expected = "note on Wait says Initiator but the edges are Responder")]
    fn sim_open_note_must_match_stored_agency() {
        let _ = crate::session_spec! {
            Hs unused [Accept, QueryReply];
            [*] --> Start
            Start --> Wait: Propose
            Wait --> End: Propose [sim_open]
            note left of Start: Initiator
            note left of Wait: Initiator
        };
    }

    #[test]
    #[should_panic(expected = "missing `note left of Wait: Initiator` or `Responder`")]
    fn sim_open_state_without_a_note_is_rejected() {
        let _ = crate::session_spec! {
            Hs unused [Accept, QueryReply];
            [*] --> Start
            Start --> Wait: Propose
            Wait --> End: Propose [sim_open]
            note left of Start: Initiator
        };
    }

    #[test]
    #[should_panic(expected = "Accept is both in the spec table and listed unused")]
    fn unused_variant_cannot_also_be_an_edge() {
        let _ = crate::session_spec! {
            Hs unused [Accept];
            [*] --> Start
            Start --> Wait: Propose
            Wait --> End: Accept
            note left of Start: Initiator
            note left of Wait: Responder
        };
    }
}
