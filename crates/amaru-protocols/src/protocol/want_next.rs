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

//! Mux `WantNext` well-formedness on an unprojected remainder graph.

use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter},
};

use amaru_pure_stage::{
    session::ProjectionConfig,
    typestate::{EffectAst, InputName, Occupancy, PayloadName, RoleName, StateName, ThenAst, TypeGraph},
};

use super::WantNext;

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
///
/// `Pull` is required on a driven state entered from the switch by a local
/// input. `drive` and the pipeliner inject it on that transition into remote
/// agency, and they do not inject it again while the instance stays remote
/// (`recv_armed`). A later remote state, such as BlockFetch `Streaming`, arms
/// `WantNext` on its wire arms instead.
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

fn want_next_name() -> PayloadName {
    WantNext::LABEL.label()
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
    let want = want_next_name();
    for e in effects {
        match e {
            EffectAst::Repeat(body) => scan_want_next(body, mux, true, scan),
            EffectAst::Send { role, payload } if *role == mux && *payload == want => {
                scan.sends += 1;
                scan.in_star |= in_star;
            }
            EffectAst::Call { role, payload } if *role == mux && *payload == want => {
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
            // `drive` / `arm_recv` inject Pull on this edge, then leave
            // `recv_armed` set until the instance returns to the switch.
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use amaru_pure_stage::{
        session::{Agency, ProjectionConfig},
        typestate::{
            EffectAst, InputName, Occupancy, PayloadName, RemainderAst, RoleName, StateName, ThenAst, TypeGraph,
        },
    };

    use super::{WantNextError, check_want_next};

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
            states: BTreeSet::from(["Idle"]),
            initial: "Idle",
            occupancy: BTreeMap::new(),
            receives: BTreeMap::from([("Idle", receives)]),
        }
    }

    #[test]
    fn driven_initiator_want_next_ok() {
        check_want_next(&initiator_graph(), &cfg_initiator()).unwrap();
    }

    #[test]
    fn undriven_responder_want_next_ok() {
        check_want_next(&responder_graph(), &cfg_responder()).unwrap();
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
    fn driven_streaming_does_not_need_its_own_pull() {
        // Streaming is entered from Busy, which is already remote, so the
        // pipeliner does not inject Pull there. The hand-built graph has none.
        assert!(!initiator_graph().receives["Streaming"].contains_key("Pull"));
        check_want_next(&initiator_graph(), &cfg_initiator()).unwrap();
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
    fn driven_missing_occupancy_is_error() {
        let mut g = initiator_graph();
        g.occupancy.remove("Busy");
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::MissingOccupancy { state: "Busy" }), "{err:?}");
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
    }

    #[test]
    fn unknown_input_is_error() {
        let mut g = initiator_graph();
        g.receives.get_mut("Busy").unwrap().insert("NotListed", seq(vec![send("ToMux", "WantNext")], "Busy"));
        let err = check_want_next(&g, &cfg_initiator()).unwrap_err();
        assert!(matches!(err, WantNextError::UnknownInput { state: "Busy", input: "NotListed" }), "{err:?}");

        let mut g = responder_graph();
        g.receives.get_mut("Idle").unwrap().insert("NotListed", seq(vec![send("ToMux", "WantNext")], "Idle"));
        let err = check_want_next(&g, &cfg_responder()).unwrap_err();
        assert!(matches!(err, WantNextError::UnknownInput { state: "Idle", input: "NotListed" }), "{err:?}");
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
}
