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

//! Drive [`Session`](amaru_pure_stage::typestate::Session) from a simulation
//! so the receive constructor and remainder actually run effects.

use amaru_pure_stage::{Effects, StageGraph, simulation::SimulationBuilder, typestate::prelude::*};

make_states!(Live { Idle; Done });

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Ping(u32);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Pong(u32);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Bye;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ClientMsg {
    Pong(Pong),
    Bye,
}

impl From<Pong> for ClientMsg {
    fn from(value: Pong) -> Self {
        ClientMsg::Pong(value)
    }
}

impl From<Bye> for ClientMsg {
    fn from(_: Bye) -> Self {
        ClientMsg::Bye
    }
}

define_role_tag!(ToClient);
define_role!(ClientDest, ToClient, ClientMsg);

on_receive!(Idle, Ping => Send<ToClient, Pong> => Idle | Send<ToClient, Bye> => Done);

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Server {
    live: Live,
    client: ClientDest,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum In {
    Ping(Ping),
}

#[test]
fn receive_then_choice_of_sends() {
    let mut network = SimulationBuilder::default();
    let client = network.stage("client", async |mut inbox: Vec<ClientMsg>, msg: ClientMsg, _eff| {
        inbox.push(msg);
        inbox
    });
    let server = network.stage("server", async |state: Server, msg: In, eff: Effects<In>| match (state.live, msg) {
        (Live::Idle(idle), In::Ping(ping)) => {
            let live = if ping.0 == 0 {
                idle.receive(ping, eff).send(&state.client, Bye).await.finish().into()
            } else {
                idle.receive(ping.clone(), eff).send(&state.client, Pong(ping.0)).await.finish().into()
            };
            Server { live, ..state }
        }
        (done @ Live::Done(_), _) => Server { live: done, ..state },
    });

    let client_ref = client.sender();
    let client = network.wire_up(client, Vec::new());
    let server =
        network.wire_up(server, Server { live: initial_state::<Idle>().into(), client: ClientDest::new(client_ref) });

    network.preload(&server, [In::Ping(Ping(7)), In::Ping(Ping(0))]).unwrap();

    let mut running = network.run();
    running.run_until_blocked().assert_idle();

    let inbox = running.get_state(&client).cloned().unwrap();
    assert_eq!(inbox, vec![ClientMsg::Pong(Pong(7)), ClientMsg::Bye]);
    assert!(matches!(running.get_state(&server).unwrap().live, Live::Done(_)));
}

make_states!(Closer { Ready; Closed });
on_receive!(Ready, Bye => Closed);

#[test]
fn empty_remainder_finishes_immediately() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("closer", async |live: Closer, msg: Bye, eff| match live {
        Closer::Ready(ready) => ready.receive(msg, eff).finish().into(),
        closed @ Closer::Closed(_) => closed,
    });
    let stage = network.wire_up(stage, initial_state::<Ready>().into());
    network.preload(&stage, [Bye]).unwrap();
    let mut running = network.run();
    running.run_until_blocked().assert_idle();
    assert!(matches!(running.get_state(&stage).cloned().unwrap(), Closer::Closed(_)));
}
