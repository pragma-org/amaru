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

#[path = "harness.rs"]
mod harness;

use amaru_pure_stage::typestate::prelude::*;
use harness::{Done, Idle, Peer, ToPeer};

on_receive!(Idle, u8 => Send<ToPeer, String> => Done);

async fn go<M: core::marker::Send>(s: Idle, peer: &Peer, eff: amaru_pure_stage::Effects<M>) {
    let _ = s.receive(1u8, eff).send(peer, 0u32).await;
}
