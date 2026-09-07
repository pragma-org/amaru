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

use std::{sync::OnceLock, time::Duration};

use amaru_pure_stage::{
    Effects, StageGraph, StageRef,
    simulation::{Run, SimulationBuilder},
    typestate::prelude::*,
};
use tokio::runtime::{Builder, Runtime};

make_states!(Live { Idle; Done });

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Ping(u32);

define_messages! {
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    enum ClientMsg {
        Pong(u32),
        Bye,
    }
}

define_role_tag!(ToClient);
define_role!(ClientDest, ToClient, ClientMsg);

define_mailbox!(In { Ping(Ping) });

on_receive!(Idle as IdleIn {
    Ping => { Send<ToClient, Pong> => Idle | Send<ToClient, Bye> => Done }
});
on_receive!(Done as DoneIn {});

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Server {
    live: Live,
    client: ClientDest,
}

#[expect(clippy::unwrap_used)]
fn test_runtime() -> &'static tokio::runtime::Handle {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Builder::new_multi_thread().enable_all().build().unwrap()).handle()
}

#[test]
fn receive_then_choice_of_sends() {
    let mut network = SimulationBuilder::default();
    let client = network.stage("client", async |mut inbox: Vec<ClientMsg>, msg: ClientMsg, _eff| {
        inbox.push(msg);
        inbox
    });
    let server = network.stage("server", async |state: Server, msg: In, eff: Effects<In>| match state.live {
        Live::Idle(idle) => match idle.convert_input(msg) {
            Ok(IdleIn::Ping(ping)) => {
                let live = if ping.0 == 0 {
                    idle.receive(ping, eff).send(&state.client, Bye).await.finish().into()
                } else {
                    idle.receive(ping.clone(), eff).send(&state.client, Pong(ping.0)).await.finish().into()
                };
                Server { live, ..state }
            }
            Err(_msg) => Server { live: idle.into(), ..state },
        },
        Live::Done(done) => match done.convert_input::<DoneIn, _>(msg) {
            Ok(never) => match never {},
            Err(_msg) => Server { live: done.into(), ..state },
        },
    });

    let client_ref = client.sender();
    let client = network.wire_up(client, Vec::new());
    let server =
        network.wire_up(server, Server { live: initial_state::<Idle>().into(), client: ClientDest::new(client_ref) });

    network.preload(&server, [Ping(7).into(), Ping(0).into()]).unwrap();

    let mut running = network.run(test_runtime());
    running.run(Run::skip_wakeups()).assert_idle();

    let inbox = running.get_state(&client).cloned().unwrap();
    assert_eq!(inbox, vec![ClientMsg::Pong(Pong(7)), ClientMsg::Bye(Bye)]);
    assert!(matches!(running.get_state(&server).unwrap().live, Live::Done(_)));
}

make_states!(Star { Open; Halt });

on_receive!(Open as OpenIn {
    Ping => { Repeat<Send<ToClient, Pong>>, Send<ToClient, Bye> => Halt }
});
on_receive!(Halt as HaltIn {});

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StarServer {
    live: Star,
    client: ClientDest,
}

#[test]
fn discard_repeat_then_send_suffix() {
    let mut network = SimulationBuilder::default();
    let client = network.stage("client", async |mut inbox: Vec<ClientMsg>, msg: ClientMsg, _eff| {
        inbox.push(msg);
        inbox
    });
    let server = network.stage("star", async |state: StarServer, msg: In, eff: Effects<In>| match state.live {
        Star::Open(open) => match open.convert_input(msg) {
            Ok(OpenIn::Ping(ping)) => {
                let n = ping.0;
                let mut session = open.receive(ping, eff);
                for _ in 0..n {
                    session = session.send(&state.client, Pong(1)).await;
                }
                let live = session.discard_repeat().send(&state.client, Bye).await.finish().into();
                StarServer { live, ..state }
            }
            Err(_msg) => StarServer { live: open.into(), ..state },
        },
        Star::Halt(halt) => match halt.convert_input::<HaltIn, _>(msg) {
            Ok(never) => match never {},
            Err(_msg) => StarServer { live: halt.into(), ..state },
        },
    });
    let client_ref = client.sender();
    let client = network.wire_up(client, Vec::new());
    let server = network
        .wire_up(server, StarServer { live: initial_state::<Open>().into(), client: ClientDest::new(client_ref) });
    network.preload(&server, [Ping(2).into()]).unwrap();
    let mut running = network.run(test_runtime());
    running.run(Run::skip_wakeups()).assert_idle();
    assert_eq!(
        running.get_state(&client).cloned().unwrap(),
        vec![ClientMsg::Pong(Pong(1)), ClientMsg::Pong(Pong(1)), ClientMsg::Bye(Bye)]
    );
    assert!(matches!(running.get_state(&server).unwrap().live, Star::Halt(_)));
}

make_states!(Closer { Ready; Closed });
define_mailbox!(CloserMsg { Bye(Bye) });
on_receive!(Ready as ReadyIn { Bye => { Closed } });
on_receive!(Closed as ClosedIn {});

#[test]
fn empty_remainder_finishes_immediately() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("closer", async |live: Closer, msg: CloserMsg, eff| match live {
        Closer::Ready(ready) => match ready.convert_input(msg) {
            Ok(ReadyIn::Bye(bye)) => ready.receive(bye, eff).finish().into(),
            Err(_msg) => ready.into(),
        },
        Closer::Closed(closed) => match closed.convert_input::<ClosedIn, _>(msg) {
            Ok(never) => match never {},
            Err(_msg) => closed.into(),
        },
    });
    let stage = network.wire_up(stage, initial_state::<Ready>().into());
    network.preload(&stage, [Bye.into()]).unwrap();
    let mut running = network.run(test_runtime());
    running.run(Run::skip_wakeups()).assert_idle();
    assert!(matches!(running.get_state(&stage), Some(Closer::Closed(_))));
}

make_states!(Watch { Quiet; Alarm, Stopped });

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Go;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Tick;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Boom;

define_mailbox!(TimedMail { Go(Go), Tick(Tick), Boom(Boom) });

on_receive!(Quiet as QuietIn {
    Go => { SetTimeout => Alarm }
});
on_receive!(Alarm as AlarmIn {
    Tick => { ClearTimeout => Stopped }
    Boom => { Stopped }
});
on_receive!(Stopped as StoppedIn {});

async fn timed_step(live: Watch, msg: TimedMail, eff: Effects<TimedMail>) -> Watch {
    match live {
        Watch::Quiet(quiet) => match quiet.convert_input(msg) {
            Ok(QuietIn::Go(go)) => {
                quiet.receive(go, eff).set_timeout(Duration::from_secs(10), Boom.into()).await.finish().into()
            }
            Err(_msg) => quiet.into(),
        },
        Watch::Alarm(alarm) => match alarm.convert_input(msg) {
            Ok(AlarmIn::Tick(tick)) => alarm.receive(tick, eff).clear_timeout().await.finish().into(),
            Ok(AlarmIn::Boom(boom)) => alarm.receive(boom, eff).finish().into(),
            Err(_msg) => alarm.into(),
        },
        Watch::Stopped(stopped) => match stopped.convert_input::<StoppedIn, _>(msg) {
            Ok(never) => match never {},
            Err(_msg) => stopped.into(),
        },
    }
}

#[test]
fn set_timeout_fires_when_not_cleared() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("timed", timed_step);
    let stage = network.wire_up(stage, initial_state::<Quiet>().into());
    network.preload(&stage, [Go.into()]).unwrap();
    let mut running = network.run(test_runtime());
    running.run(Run::default()).assert_sleeping();
    assert!(matches!(running.get_state(&stage).unwrap(), Watch::Alarm(_)));
    let blocked = running.run(Run::skip_wakeups());
    assert_eq!(blocked, amaru_pure_stage::simulation::Blocked::Idle);
    assert!(matches!(running.get_state(&stage), Some(Watch::Stopped(_))));
}

#[test]
fn clear_timeout_prevents_the_message() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("timed", timed_step);
    let stage = network.wire_up(stage, initial_state::<Quiet>().into());
    network.preload(&stage, [Go.into()]).unwrap();
    let mut running = network.run(test_runtime());
    running.run(Run::default()).assert_sleeping();
    running.enqueue_msg(&stage, [Tick.into()]);
    running.run(Run::skip_wakeups()).assert_idle();
    assert!(matches!(running.get_state(&stage), Some(Watch::Stopped(_))));
    assert!(!running.skip_to_next_wakeup(None));
}

make_states!(Rpc { Asking; Answered });

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum EchoMail {
    Ask(u32, StageRef<u32>),
}

define_role_tag!(ToEcho);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct EchoDest(StageRef<EchoMail>);

impl EchoDest {
    fn new(stage: StageRef<EchoMail>) -> Self {
        Self(stage)
    }
}

impl Role<ToEcho> for EchoDest {
    type Mailbox = EchoMail;

    fn mailbox(&self) -> &StageRef<EchoMail> {
        &self.0
    }
}

impl IntoRoleCall<ToEcho, u32> for EchoDest {
    type Reply = u32;
    const TIMEOUT: Duration = Duration::from_secs(1);

    fn encode(&self, n: u32, reply: StageRef<u32>) -> EchoMail {
        EchoMail::Ask(n, reply)
    }
}

on_receive!(Asking as AskingIn {
    Ping => { Call<ToEcho, u32> => Answered }
});
on_receive!(Answered as AnsweredIn {});

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct RpcServer {
    live: Rpc,
    echo: EchoDest,
    got: Option<u32>,
}

#[test]
fn call_waits_for_the_reply() {
    let mut network = SimulationBuilder::default();
    let echo = network.stage("echo", async |_: (), msg: EchoMail, eff: Effects<EchoMail>| {
        let EchoMail::Ask(n, reply) = msg;
        eff.send(&reply, n * 2).await;
    });
    let server = network.stage("rpc", async |state: RpcServer, msg: In, eff: Effects<In>| match state.live {
        Rpc::Asking(asking) => match asking.convert_input(msg) {
            Ok(AskingIn::Ping(ping)) => {
                let n = ping.0;
                let (reply, s) = asking.receive(ping, eff).call(&state.echo, n).await;
                RpcServer { live: s.finish().into(), got: reply, echo: state.echo }
            }
            Err(_msg) => RpcServer { live: asking.into(), ..state },
        },
        Rpc::Answered(answered) => match answered.convert_input::<AnsweredIn, _>(msg) {
            Ok(never) => match never {},
            Err(_msg) => RpcServer { live: answered.into(), ..state },
        },
    });
    let echo_ref = echo.sender();
    let _echo = network.wire_up(echo, ());
    let server = network.wire_up(
        server,
        RpcServer { live: initial_state::<Asking>().into(), echo: EchoDest::new(echo_ref), got: None },
    );
    network.preload(&server, [Ping(7).into()]).unwrap();
    let mut running = network.run(test_runtime());
    running.run(Run::skip_wakeups()).assert_idle();
    let state = running.get_state(&server).unwrap();
    assert_eq!(state.got, Some(14));
    assert!(matches!(state.live, Rpc::Answered(_)));
}
