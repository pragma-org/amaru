// Copyright 2025 PRAGMA
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

use amaru_kernel::ProtocolVersion;
use chumsky::{extra::SimpleState, input, prelude::*};

use crate::{arena::Arena, flat::Ctx, machine::MachineVersion};

pub struct State<'a> {
    pub arena: &'a Arena,
    pub env: Vec<&'a str>,
    pub machine_version: MachineVersion,
    pub protocol_version: ProtocolVersion,
}

impl<'a> State<'a> {
    pub fn new(arena: &'a Arena, protocol_version: ProtocolVersion) -> Self {
        Self { arena, env: Vec::new(), machine_version: MachineVersion::default(), protocol_version }
    }

    pub fn set_machine_version(&mut self, machine_version: MachineVersion) {
        self.machine_version = machine_version;
    }

    pub fn is_constr_case_available(&self) -> bool {
        Ctx { arena: self.arena, machine_version: self.machine_version, protocol_version: self.protocol_version }
            .is_constr_case_available()
    }
}

pub type Extra<'a> = extra::Full<Rich<'a, char>, SimpleState<State<'a>>, ()>;

pub type MapExtra<'a, 'b> = input::MapExtra<'a, 'b, &'a str, Extra<'a>>;
