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

//! Undirected session spec and mux projection of typestate remainder graphs.
//!
//! [`SessionSpec`] stores the network-spec table (who sends, not who we are).
//! [`SessionSpec::project`] orients that table for one [`Role`]. Handler
//! [`project`] hides mux plumbing, timers, and local roles, unfolding remainder
//! sequences onto a [`Cfsm`]. Comparisons run in tests and panic on mismatch.

#![expect(clippy::panic, clippy::unwrap_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Formatter, Write as _},
    time::Duration,
};

use amaru_pure_stage::typestate::{
    EffectAst, InputName, Occupancy, PayloadName, RoleName, StateName, ThenAst, TypeGraph,
};

use super::Role;

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
pub struct Label<M> {
    pub direction: Direction,
    pub message: M,
}

impl<M> Label<M> {
    fn send(message: M) -> Self {
        Self { direction: Direction::Send, message }
    }

    fn recv(message: M) -> Self {
        Self { direction: Direction::Recv, message }
    }
}

impl<M: Display> Display for Label<M> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.direction.as_bang_query(), self.message)
    }
}

/// Oriented exclusive-agency machine. Timeouts live on [`SessionSpec`], not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cfsm<M> {
    pub states: BTreeSet<StateId>,
    pub initial: StateId,
    pub terminal: BTreeSet<StateId>,
    /// Who may send. Omitted for [`terminal`](Self::terminal) states.
    pub agency: BTreeMap<StateId, Role>,
    pub transitions: BTreeMap<StateId, BTreeMap<Label<M>, StateId>>,
}

impl<M> Cfsm<M>
where
    M: Clone + Ord + std::fmt::Debug,
{
    /// Panic on mismatch. Compares reachable oriented transitions and `agency`.
    /// Does **not** compare timeouts.
    #[track_caller]
    pub fn assert_refines(&self, spec: &Cfsm<M>, map: impl Fn(&StateId) -> StateId) {
        let got = self.reachable_fragment().collapse(map);
        let want = spec.reachable_fragment();
        if got.transitions != want.transitions || got.agency != want.agency {
            panic!("assert_refines mismatch\nprojected:\n{}\n\nspec:\n{}", got.fmt_table(), want.fmt_table());
        }
    }

    /// Reachable-table equality (transitions + agency + terminal). No search.
    #[track_caller]
    pub fn assert_bisimilar(&self, other: &Cfsm<M>) {
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
    #[must_use]
    pub fn dual(&self) -> Cfsm<M> {
        let transitions = self
            .transitions
            .iter()
            .map(|(from, edges)| {
                let swapped = edges
                    .iter()
                    .map(|(label, to)| {
                        (Label { direction: label.direction.opposite(), message: label.message.clone() }, to.clone())
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

    /// Apply a declared surjection; panic if two sources map to one dest with disagreeing labels.
    #[must_use]
    #[track_caller]
    pub fn collapse(&self, map: impl Fn(&StateId) -> StateId) -> Cfsm<M> {
        let mut states = BTreeSet::new();
        let mut transitions: BTreeMap<StateId, BTreeMap<Label<M>, StateId>> = BTreeMap::new();
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
                    panic!("collapse: {from2} --{label:?}--> {to2} already defined as {existing} (disagreeing labels)");
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
    pub fn retarget(&self, done: &M, new_to: StateId) -> Cfsm<M> {
        let mut out = self.clone();
        out.states.insert(new_to.clone());
        for edges in out.transitions.values_mut() {
            for (label, to) in edges.iter_mut() {
                if &label.message == done {
                    *to = new_to.clone();
                }
            }
        }
        out.recompute_terminal();
        out
    }

    /// Destination of the unique edge from `from` whose message is `msg`.
    #[track_caller]
    pub fn dest(&self, from: StateId, msg: &M) -> StateId {
        let Some(edges) = self.transitions.get(&from) else {
            panic!("dest: no transitions from {from}");
        };
        let mut found = None;
        for (label, to) in edges {
            if &label.message == msg {
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

    fn reachable_fragment(&self) -> Cfsm<M> {
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

    fn fmt_table(&self) -> String
    where
        M: std::fmt::Debug,
    {
        let mut s = String::new();
        let _ = writeln!(s, "initial: {}", self.initial);
        for state in &self.states {
            let agency = self.agency.get(state);
            let terminal = self.terminal.contains(state);
            let _ = writeln!(s, "{state} agency={agency:?} terminal={terminal}");
            if let Some(edges) = self.transitions.get(state) {
                for (label, to) in edges {
                    let _ = writeln!(s, "  {label:?} -> {to}");
                }
            }
        }
        s
    }
}

/// Same shape as `ProtoSpec` without `ProtocolState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec<S, M> {
    transitions: BTreeMap<S, PerState<S, M>>,
    /// Receiver's bound. Absence = no timer.
    timeout: BTreeMap<S, Duration>,
    /// `from` of the first `init` / `resp` / `sim_open`.
    initial: Option<S>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PerState<S, M> {
    agency: Role,
    transitions: BTreeMap<M, Edge<S>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Edge<S> {
    sender: Role,
    to: S,
    sim_open: bool,
}

impl<S, M> Default for SessionSpec<S, M> {
    fn default() -> Self {
        Self { transitions: BTreeMap::new(), timeout: BTreeMap::new(), initial: None }
    }
}

impl<S, M> PerState<S, M> {
    fn initiator() -> Self {
        Self { agency: Role::Initiator, transitions: BTreeMap::new() }
    }

    fn responder() -> Self {
        Self { agency: Role::Responder, transitions: BTreeMap::new() }
    }

    fn insert(&mut self, msg: M, role: Role, to: S, sim_open: bool) -> Option<Edge<S>>
    where
        M: std::fmt::Debug + Ord,
        S: std::fmt::Debug,
    {
        assert_eq!(self.agency, role, "inserting {msg:?}@{role:?} to {to:?}");
        self.transitions.insert(msg, Edge { sender: role, to, sim_open })
    }
}

impl<S, M> SessionSpec<S, M>
where
    S: Clone + Ord + std::fmt::Debug,
    M: Clone + Ord + std::fmt::Debug,
{
    /// Add a transition that the initiator sends.
    pub fn init(&mut self, from: S, msg: M, to: S) {
        self.insert_edge(from, msg, to, Role::Initiator, false);
    }

    /// Add a transition that the responder sends.
    pub fn resp(&mut self, from: S, msg: M, to: S) {
        self.insert_edge(from, msg, to, Role::Responder, false);
    }

    /// Simultaneous-open alias: Recv of `msg` for the waiting role, not mixed agency.
    pub fn sim_open(&mut self, from: S, msg: M, to: S) {
        self.insert_edge(from, msg, to, Role::Responder, true);
    }

    pub fn set_timeout(&mut self, state: S, d: Duration) {
        self.timeout.insert(state, d);
    }

    /// Receiver timeout for `state`, if any.
    pub fn timeout(&self, state: &S) -> Option<Duration> {
        self.timeout.get(state).copied()
    }

    fn insert_edge(&mut self, from: S, msg: M, to: S, role: Role, sim_open: bool) {
        if self.initial.is_none() {
            self.initial = Some(from.clone());
        }
        let per = self.transitions.entry(from.clone()).or_insert_with(|| match role {
            Role::Initiator => PerState::initiator(),
            Role::Responder => PerState::responder(),
        });
        if let Some(present) = per.insert(msg.clone(), role, to.clone(), sim_open) {
            panic!("transition {from:?} -> {msg:?} -> {present:?} already defined when inserting {to:?}");
        }
    }

    /// Library helper for protocols whose remainders still loop `MsgDone`.
    /// Retargets only the undirected done-edge destinations.
    #[must_use]
    pub fn with_restart_on_done(mut self, done: M, to: S) -> Self {
        for per in self.transitions.values_mut() {
            for (msg, edge) in per.transitions.iter_mut() {
                if msg == &done {
                    edge.to = to.clone();
                }
            }
        }
        self
    }

    /// Orient: from a state where `role == agency`, outgoing labels are Send; otherwise Recv.
    /// `sim_open` edges are Recv of the aliased message for the waiting role.
    pub fn project(&self, role: Role) -> Cfsm<M>
    where
        S: Into<StateName>,
    {
        let Some(initial) = &self.initial else {
            panic!("SessionSpec::project on empty spec");
        };
        let named = |s: &S| StateId::Named(s.clone().into());

        let mut states = BTreeSet::new();
        let mut transitions: BTreeMap<StateId, BTreeMap<Label<M>, StateId>> = BTreeMap::new();
        let mut agency = BTreeMap::new();

        for (from, per) in &self.transitions {
            let from_id = named(from);
            states.insert(from_id.clone());
            let mut edges = BTreeMap::new();
            for (msg, edge) in &per.transitions {
                let to_id = named(&edge.to);
                states.insert(to_id.clone());
                let direction = if edge.sim_open {
                    // Waiting role only; skip on the agency holder so labels stay exclusive.
                    if role == per.agency {
                        continue;
                    }
                    Direction::Recv
                } else if role == per.agency {
                    Direction::Send
                } else {
                    Direction::Recv
                };
                edges.insert(Label { direction, message: msg.clone() }, to_id);
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

/// Hand-written per protocol. `driven` is stored for later WantNext/timeout checkers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionConfig<M> {
    pub role: Role,
    pub peer_role: RoleName,
    pub mux_role: RoleName,
    pub local_roles: BTreeSet<RoleName>,
    /// Receive-arm identifiers (`stringify!($in)`) → spec message.
    pub wire_inputs: BTreeMap<InputName, M>,
    /// Remainder `Call`/`Send` payload last-segments → the same dummy `M` as the spec table.
    pub wire_payload: BTreeMap<PayloadName, M>,
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
    EmptyExpandSeq { origin: StateId },
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
            ProjectError::EmptyExpandSeq { origin } => {
                write!(f, "expand_seq invoked on an all-hidable sequence at {origin}")
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
pub fn project<M>(graph: &TypeGraph, cfg: &ProjectionConfig<M>) -> Result<Cfsm<M>, ProjectError>
where
    M: Clone + Ord + std::fmt::Debug,
{
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
                if hidable_only.is_empty() && wire_bearing.is_empty() {
                    return Err(ProjectError::NoWireFromLocal { state, input });
                }
                if !hidable_only.is_empty() {
                    return Err(ProjectError::NoWireFromLocal { state, input });
                }
                let origin = StateId::Named(state);
                for (seq, next) in wire_bearing {
                    proj.expand_seq(origin.clone(), &seq, next)?;
                }
                continue;
            }

            if let Some(m) = cfg.wire_inputs.get(input) {
                if !hidable_only.is_empty() && !wire_bearing.is_empty() {
                    return Err(ProjectError::MixedHidableWireChoice { state, input });
                }
                if !hidable_only.is_empty() {
                    let mut nexts = BTreeSet::new();
                    for alt in hidable_only {
                        nexts.insert(alt.next);
                    }
                    if nexts.len() != 1 {
                        return Err(ProjectError::Nondeterministic {
                            state: StateId::Named(state),
                            label: format!("Recv({m:?})"),
                        });
                    }
                    let next = *nexts.iter().next().unwrap();
                    proj.emit(StateId::Named(state), Label::recv(m.clone()), StateId::Named(next))?;
                } else if !wire_bearing.is_empty() {
                    let payload = proj.payload_name_of(m).unwrap_or(input);
                    let dest = next_synthetic(&StateId::Named(state), payload);
                    proj.emit(StateId::Named(state), Label::recv(m.clone()), dest.clone())?;
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
    drop_unreachable_synthetics(&mut cfsm);
    cfsm.recompute_terminal();
    Ok(cfsm)
}

struct Projector<'a, M> {
    cfg: &'a ProjectionConfig<M>,
    states: BTreeSet<StateId>,
    transitions: BTreeMap<StateId, BTreeMap<Label<M>, StateId>>,
}

impl<M> Projector<'_, M>
where
    M: Clone + Ord + std::fmt::Debug,
{
    fn hide_parallel(&self, state: StateName, input: InputName, alt: &ThenAst) -> Result<Vec<EffectAst>, ProjectError> {
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

    fn hide_seq(&self, seq: &[EffectAst]) -> Result<Vec<EffectAst>, ProjectError> {
        let mut out = Vec::new();
        for e in seq {
            if !self.is_hidable(e)? {
                out.push(e.clone());
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
                    if self.cfg.wire_payload.contains_key(payload) {
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

    fn expand_seq(&mut self, origin: StateId, seq: &[EffectAst], named_next: StateName) -> Result<(), ProjectError> {
        let mut i = 0;
        while i < seq.len() && self.is_hidable(&seq[i])? {
            i += 1;
        }
        if i == seq.len() {
            return Err(ProjectError::EmptyExpandSeq { origin });
        }
        let mut state = origin;
        while i < seq.len() {
            let e = &seq[i];
            if self.is_hidable(e)? {
                i += 1;
                continue;
            }
            match e {
                EffectAst::Repeat(body) => {
                    let body = self.non_hidable_flat(body)?;
                    if body.is_empty() {
                        i += 1;
                        continue;
                    }
                    if let Some(m) = self.single_wire_send(&body) {
                        if self.first_wire(&seq[i + 1..])?.is_some_and(|head| head == m) {
                            return Err(ProjectError::AmbiguousRepeat { origin: state });
                        }
                        self.emit(state.clone(), Label::send(m.clone()), state.clone())?;
                        i += 1;
                        continue;
                    }
                    return Err(ProjectError::RepeatStarTooWide { origin: state });
                }
                EffectAst::SendAny { role } => {
                    if *role == self.cfg.peer_role {
                        return Err(ProjectError::PeerSendAny { role });
                    }
                    return Err(ProjectError::UnknownRole { role });
                }
                EffectAst::Call { role, payload } | EffectAst::Send { role, payload } => {
                    if *role != self.cfg.peer_role {
                        return Err(ProjectError::UnknownRole { role });
                    }
                    let Some(m) = self.cfg.wire_payload.get(payload) else {
                        return Err(ProjectError::UnknownPeerPayload { payload });
                    };
                    if !self.has_remaining_non_hidable(&seq[i + 1..])? {
                        self.emit(state, Label::send(m.clone()), StateId::Named(named_next))?;
                        return Ok(());
                    }
                    let dest = next_synthetic(&state, payload);
                    self.emit(state, Label::send(m.clone()), dest.clone())?;
                    state = dest;
                    i += 1;
                }
                EffectAst::SetTimeout
                | EffectAst::ClearTimeout
                | EffectAst::Wait
                | EffectAst::Terminate
                | EffectAst::Clock
                | EffectAst::Schedule { .. }
                | EffectAst::CancelSchedule
                | EffectAst::External { .. }
                | EffectAst::AddStage => {
                    i += 1;
                }
            }
        }
        Ok(())
    }

    fn non_hidable_flat(&self, body: &[EffectAst]) -> Result<Vec<EffectAst>, ProjectError> {
        let mut flat = Vec::new();
        for e in body {
            if let EffectAst::Repeat(inner) = e {
                flat.extend(inner.iter().cloned());
            } else {
                flat.push(e.clone());
            }
        }
        let mut out = Vec::new();
        for e in flat {
            if !self.is_hidable(&e)? {
                out.push(e);
            }
        }
        Ok(out)
    }

    fn single_wire_send<'a>(&'a self, body: &'a [EffectAst]) -> Option<&'a M> {
        if body.len() != 1 {
            return None;
        }
        self.as_wire_send(&body[0])
    }

    fn as_wire_send<'a>(&'a self, e: &'a EffectAst) -> Option<&'a M> {
        match e {
            EffectAst::Call { role, payload } | EffectAst::Send { role, payload } => {
                if *role == self.cfg.peer_role {
                    self.cfg.wire_payload.get(payload)
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

    fn first_wire<'a>(&'a self, suffix: &'a [EffectAst]) -> Result<Option<&'a M>, ProjectError> {
        for e in suffix {
            if self.is_hidable(e)? {
                continue;
            }
            return Ok(self.as_wire_send(e));
        }
        Ok(None)
    }

    fn has_remaining_non_hidable(&self, suffix: &[EffectAst]) -> Result<bool, ProjectError> {
        for e in suffix {
            if !self.is_hidable(e)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn payload_name_of(&self, m: &M) -> Option<PayloadName> {
        self.cfg.wire_payload.iter().find_map(|(name, v)| (v == m).then_some(*name))
    }

    fn emit(&mut self, from: StateId, label: Label<M>, to: StateId) -> Result<(), ProjectError> {
        self.states.insert(from.clone());
        self.states.insert(to.clone());
        let edges = self.transitions.entry(from.clone()).or_default();
        if let Some(existing) = edges.get(&label)
            && existing != &to
        {
            return Err(ProjectError::Nondeterministic { state: from, label: format!("{label:?}") });
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

fn drop_unreachable_synthetics<M>(cfsm: &mut Cfsm<M>)
where
    M: Clone + Ord + std::fmt::Debug,
{
    let reach = cfsm.reachable();
    cfsm.states.retain(|s| match s {
        StateId::Named(_) => true,
        StateId::Synthetic { .. } => reach.contains(s),
    });
    cfsm.transitions.retain(|s, _| cfsm.states.contains(s));
    cfsm.agency.retain(|s, _| cfsm.states.contains(s));
}

#[cfg(test)]
mod tests {
    use amaru_pure_stage::typestate::RemainderAst;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Msg {
        RequestRange,
        ClientDone,
        StartBatch,
        NoBlocks,
        Block,
        BatchDone,
    }

    impl Display for Msg {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.write_str(match self {
                Msg::RequestRange => "RequestRange",
                Msg::ClientDone => "ClientDone",
                Msg::StartBatch => "StartBatch",
                Msg::NoBlocks => "NoBlocks",
                Msg::Block => "Block",
                Msg::BatchDone => "BatchDone",
            })
        }
    }

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

    fn payloads() -> BTreeMap<PayloadName, Msg> {
        BTreeMap::from([
            ("RequestRange", Msg::RequestRange),
            ("ClientDone", Msg::ClientDone),
            ("StartBatch", Msg::StartBatch),
            ("NoBlocks", Msg::NoBlocks),
            ("Block", Msg::Block),
            ("BatchDone", Msg::BatchDone),
        ])
    }

    fn table_37() -> SessionSpec<&'static str, Msg> {
        let mut spec = SessionSpec::default();
        spec.init("Idle", Msg::RequestRange, "Busy");
        spec.init("Idle", Msg::ClientDone, "Done");
        spec.resp("Busy", Msg::NoBlocks, "Idle");
        spec.resp("Busy", Msg::StartBatch, "Streaming");
        spec.resp("Streaming", Msg::Block, "Streaming");
        spec.resp("Streaming", Msg::BatchDone, "Idle");
        spec.set_timeout("Busy", Duration::from_secs(60));
        spec.set_timeout("Streaming", Duration::from_secs(60));
        spec
    }

    fn cfg_initiator() -> ProjectionConfig<Msg> {
        ProjectionConfig {
            role: Role::Initiator,
            peer_role: "ToResponder",
            mux_role: "ToMux",
            local_roles: BTreeSet::from(["ToCollector"]),
            wire_inputs: BTreeMap::from([
                ("StartBatch", Msg::StartBatch),
                ("NoBlocks", Msg::NoBlocks),
                ("Block", Msg::Block),
                ("BatchDone", Msg::BatchDone),
            ]),
            wire_payload: payloads(),
            plumbing_inputs: BTreeSet::from(["Pull"]),
            local_inputs: BTreeSet::from(["Fetch", "Close"]),
            driven: true,
        }
    }

    fn cfg_responder() -> ProjectionConfig<Msg> {
        ProjectionConfig {
            role: Role::Responder,
            peer_role: "ToInitiator",
            mux_role: "ToMux",
            local_roles: BTreeSet::new(),
            wire_inputs: BTreeMap::from([("RequestRange", Msg::RequestRange), ("ClientDone", Msg::ClientDone)]),
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
        let spec = table_37().project(Role::Initiator);
        let got = project(&initiator_graph(), &cfg_initiator()).unwrap();
        assert!(got.states.iter().all(|s| matches!(s, StateId::Named(_))));
        got.assert_refines(&spec, identity);
        assert_eq!(got.agency.get(&named("Idle")), Some(&Role::Initiator));
        assert_eq!(got.agency.get(&named("Busy")), Some(&Role::Responder));
        assert_eq!(got.agency.get(&named("Streaming")), Some(&Role::Responder));
        assert!(got.terminal.contains(&named("Done")));
        assert_eq!(got.dest(named("Idle"), &Msg::RequestRange), named("Busy"));
        assert_eq!(got.dest(named("Idle"), &Msg::ClientDone), named("Done"));
        assert_eq!(got.dest(named("Busy"), &Msg::NoBlocks), named("Idle"));
        assert_eq!(got.dest(named("Busy"), &Msg::StartBatch), named("Streaming"));
        assert_eq!(got.dest(named("Streaming"), &Msg::Block), named("Streaming"));
        assert_eq!(got.dest(named("Streaming"), &Msg::BatchDone), named("Idle"));
    }

    #[test]
    fn responder_request_range_synthetics_refine_spec() {
        let spec = table_37().project(Role::Responder);
        let got = project(&responder_graph(), &cfg_responder()).unwrap();
        let req = StateId::Synthetic { parent: "Idle", path: vec!["RequestRange"] };
        let start = StateId::Synthetic { parent: "Idle", path: vec!["RequestRange", "StartBatch"] };
        assert_eq!(got.dest(named("Idle"), &Msg::RequestRange), req);
        assert_eq!(got.dest(req.clone(), &Msg::StartBatch), start);
        assert_eq!(got.dest(req.clone(), &Msg::NoBlocks), named("Idle"));
        assert_eq!(got.dest(start.clone(), &Msg::Block), start);
        assert_eq!(got.dest(start, &Msg::BatchDone), named("Idle"));
        assert_eq!(got.dest(named("Idle"), &Msg::ClientDone), named("Done"));
        got.assert_refines(&spec, map_responder);
        got.collapse(map_responder).assert_bisimilar(&spec);
    }

    #[test]
    fn dual_of_spec_initiator_equals_spec_responder() {
        let spec = table_37();
        let spec_i = spec.project(Role::Initiator);
        let spec_r = spec.project(Role::Responder);
        spec_i.dual().assert_bisimilar(&spec_r);
        assert_eq!(spec_i.agency.get(&named("Idle")), Some(&Role::Initiator));
        assert_eq!(spec_r.agency.get(&named("Idle")), Some(&Role::Initiator));
        assert_eq!(spec_i.dual().agency.get(&named("Idle")), Some(&Role::Initiator));
        assert_eq!(spec_i.agency.get(&named("Busy")), Some(&Role::Responder));
        assert_eq!(spec_r.agency.get(&named("Busy")), Some(&Role::Responder));
    }

    #[test]
    fn with_restart_on_done_retargets_only_the_done_edge() {
        let spec = table_37().with_restart_on_done(Msg::ClientDone, "Idle");
        let cfsm = spec.project(Role::Initiator);
        assert_eq!(cfsm.dest(named("Idle"), &Msg::ClientDone), named("Idle"));
        assert_eq!(cfsm.dest(named("Idle"), &Msg::RequestRange), named("Busy"));
        assert_eq!(cfsm.dest(named("Busy"), &Msg::StartBatch), named("Streaming"));
        assert_eq!(spec.timeout(&"Busy"), Some(Duration::from_secs(60)));
        let original = table_37().project(Role::Initiator);
        assert_eq!(original.dest(named("Idle"), &Msg::ClientDone), named("Done"));
        let retargeted = original.retarget(&Msg::ClientDone, named("Idle"));
        assert_eq!(retargeted.dest(named("Idle"), &Msg::ClientDone), named("Idle"));
        assert_eq!(retargeted.dest(named("Idle"), &Msg::RequestRange), named("Busy"));
    }

    #[test]
    fn assert_refines_ignores_timeouts() {
        let mut timed = table_37();
        timed.set_timeout("Idle", Duration::from_secs(1));
        timed.project(Role::Initiator).assert_refines(&table_37().project(Role::Initiator), identity);
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
        assert_eq!(got.dest(syn.clone(), &Msg::Block), syn);
        assert_eq!(got.dest(syn, &Msg::BatchDone), named("Idle"));
    }

    #[test]
    fn projection_errors() {
        struct Case {
            name: &'static str,
            graph: TypeGraph,
            cfg: ProjectionConfig<Msg>,
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
        ];

        for case in cases {
            let err = project(&case.graph, &case.cfg).expect_err(case.name);
            assert!((case.check)(&err), "{}: unexpected {err:?}", case.name);
        }
    }
}
