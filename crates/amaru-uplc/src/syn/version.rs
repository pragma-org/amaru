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

use chumsky::{Parser, prelude::*};

use super::types::{Extra, MapExtra};
use crate::machine::MachineVersion;

pub fn parser<'a>() -> impl Parser<'a, &'a str, MachineVersion, Extra<'a>> {
    text::int(10)
        .map(|v: &str| v.parse().unwrap())
        .then_ignore(just('.'))
        .then(text::int(10).map(|v: &str| v.parse().unwrap()))
        .then_ignore(just('.'))
        .then(text::int(10).map(|v: &str| v.parse().unwrap()))
        .map_with(|((major, minor), patch), e: &mut MapExtra<'a, '_>| {
            let state = e.state();

            let version = MachineVersion::new(major, minor, patch);

            state.set_machine_version(version);

            version
        })
}
