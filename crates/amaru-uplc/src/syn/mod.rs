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

use amaru_kernel::{ProtocolVersion, protocol_version};
use chumsky::{ParseResult, Parser, extra::SimpleState, prelude::*};

mod constant;
mod data;
mod program;
mod term;
mod typ;
mod types;
mod utils;
mod version;

use crate::{arena::Arena, binder::DeBruijn, constant::Constant, data::PlutusData, program::Program, term::Term};

pub fn parse_program<'a>(
    arena: &'a Arena,
    input: &'a str,
    protocol_version: ProtocolVersion,
) -> ParseResult<&'a Program<'a, DeBruijn>, Rich<'a, char>> {
    let mut initial_state = SimpleState(types::State::new(arena, protocol_version));
    program::parser().parse_with_state(input, &mut initial_state)
}

pub fn parse_term<'a>(arena: &'a Arena, input: &'a str) -> ParseResult<&'a Term<'a, DeBruijn>, Rich<'a, char>> {
    let mut initial_state = SimpleState(types::State::new(arena, protocol_version::DEFAULT));
    term::parser().parse_with_state(input, &mut initial_state)
}

pub fn parse_constant<'a>(arena: &'a Arena, input: &'a str) -> ParseResult<&'a Constant<'a>, Rich<'a, char>> {
    let mut initial_state = SimpleState(types::State::new(arena, protocol_version::DEFAULT));
    constant::parser().parse_with_state(input, &mut initial_state)
}

pub fn parse_data<'a>(arena: &'a Arena, input: &'a str) -> ParseResult<&'a PlutusData<'a>, Rich<'a, char>> {
    let mut initial_state = SimpleState(types::State::new(arena, protocol_version::DEFAULT));
    data::parser().parse_with_state(input, &mut initial_state)
}
