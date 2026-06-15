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

use chumsky::{extra::SimpleState, input, prelude::*};

use crate::{arena::Arena, program::Version};

pub struct State<'a> {
    pub arena: &'a Arena,
    pub env: Vec<&'a str>,
    pub version: Option<Version<'a>>,
    pub protocol_version: Option<u32>,
}

impl<'a> State<'a> {
    pub fn new(arena: &'a Arena, protocol_version: Option<u32>) -> Self {
        Self { arena, env: Vec::new(), version: None, protocol_version }
    }

    pub fn set_version(&mut self, version: Version<'a>) {
        self.version = Some(version);
    }

    pub fn is_constr_case_available(&self) -> bool {
        let protocol_ok = self.protocol_version.is_none_or(|pv| pv >= 9);
        let version_ok = self.version.is_none_or(|v| v.is_constr_case_available());
        protocol_ok && version_ok
    }
}

pub type Extra<'a> = extra::Full<Rich<'a, char>, SimpleState<State<'a>>, ()>;
pub type MapExtra<'a, 'b> = input::MapExtra<'a, 'b, &'a str, Extra<'a>>;
