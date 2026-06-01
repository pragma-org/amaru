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

use std::time::Duration;

use self::chainsync::*;
use crate::{
    StageRef,
    typestate::{Transition, type_lists::initial_state},
};

/// This illustrates writing a function that performs some protocol step (it could easily
/// do multiple steps by starting a new transition).
#[expect(unused)]
pub async fn intersect(s: Idle, mux: &StageRef<String>, other: &StageRef<u8>, eff: crate::Effects<()>) -> Intersect {
    let e = s.start(eff);

    let e = e.send(mux, "intersect".to_string()).await;
    let e = e.wait(Duration::from_secs(1)).await;
    let e = e.send(other, 42).await;

    e.finish()
}

#[test]
fn test_intersect() {
    // illustrate how to construct the initial state
    let _s = initial_state::<Idle>();
    // let s = initial_state::<Intersect>();
    assert_eq!(
        // since the state machine is fully declared at the type level, we can e.g.
        // print a transition without having either of the states constructed as a value.
        <Idle as Transition<Intersect>>::to_string(),
        "Idle -> Send<alloc::string::String>, Send<u8> | Wait -> Intersect"
    );
}

mod chainsync {
    use crate::{effects, make_states, transition, typestate::prelude::*};

    // First declare the states, with initial state(s) before the semicolon.
    make_states!(Idle; Intersect, Done, CanAwait, MustSend);

    // Then declare the transitions with the effect sequences they require.
    // Note that this is a toy example using the structure of the chainsync
    // protocol, but we don’t have the message types available here.

    transition!(Idle => Intersect: effects!(Send<String>, Send<u8> | Wait));
    transition!(Intersect => Idle: effects!(Receive<u8>));

    transition!(Idle => CanAwait: effects!(Send<()>));
    transition!(CanAwait => Idle: effects!(Receive<String>));
    transition!(CanAwait => MustSend: effects!(Receive<()>));
    transition!(MustSend => Idle: effects!(Receive<String>));

    transition!(Idle => Done: effects!(Send<()>));
}
