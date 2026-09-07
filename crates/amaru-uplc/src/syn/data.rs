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

use bumpalo::collections::Vec as BumpVec;
use chumsky::prelude::*;
use num::Num;

use super::{
    types::{Extra, MapExtra},
    utils::hex_bytes,
};
use crate::{constant::Integer, data::PlutusData};

pub fn parser<'a>() -> impl Parser<'a, &'a str, &'a PlutusData<'a>, Extra<'a>> {
    recursive(|data| {
        choice((
            just('B').padded().ignore_then(just('#').ignore_then(hex_bytes()).padded()).map_with(
                |v, e: &mut MapExtra<'a, '_>| {
                    let state = e.state();

                    let bytes = BumpVec::from_iter_in(v, state.arena.as_bump());
                    let bytes = state.arena.alloc(bytes);

                    PlutusData::byte_string(state.arena, bytes)
                },
            ),
            just('I')
                .padded()
                .ignore_then(just('-').ignored().or_not().padded().then(text::int(10)).padded())
                .map_with(|(maybe_negative, v), e: &mut MapExtra<'a, '_>| {
                    let state = e.state();

                    #[expect(clippy::unwrap_used)]
                    let mut i = Integer::from_str_radix(v, 10).unwrap();

                    if maybe_negative.is_some() {
                        i = -i;
                    };

                    let value = state.arena.alloc_integer(i);

                    PlutusData::integer(state.arena, value)
                }),
            just("Constr")
                .padded()
                .ignore_then(text::int(10).padded())
                .then(data.clone().separated_by(just(',').padded()).collect().delimited_by(just('['), just(']')))
                .map_with(|(tag, fields): (_, Vec<&PlutusData<'_>>), e: &mut MapExtra<'a, '_>| {
                    let state = e.state();

                    let fields = BumpVec::from_iter_in(fields, state.arena.as_bump());
                    let fields = state.arena.alloc(fields);

                    #[expect(clippy::unwrap_used)]
                    PlutusData::constr(state.arena, tag.parse().unwrap(), fields)
                }),
            just("List")
                .padded()
                .ignore_then(data.clone().separated_by(just(',').padded()).collect().delimited_by(just('['), just(']')))
                .map_with(|items: Vec<&PlutusData<'_>>, e: &mut MapExtra<'a, '_>| {
                    let state = e.state();

                    let fields = BumpVec::from_iter_in(items, state.arena.as_bump());
                    let fields = state.arena.alloc(fields);

                    PlutusData::list(state.arena, fields)
                }),
            just("Map")
                .padded()
                .ignore_then(
                    data.clone()
                        .padded()
                        .then_ignore(just(',').padded())
                        .then(data.padded())
                        .delimited_by(just('('), just(')'))
                        .separated_by(just(',').padded())
                        .collect()
                        .padded()
                        .delimited_by(just('['), just(']'))
                        .padded(),
                )
                .map_with(|items: Vec<(&PlutusData<'_>, &PlutusData<'_>)>, e: &mut MapExtra<'a, '_>| {
                    let state = e.state();

                    let fields = BumpVec::from_iter_in(items, state.arena.as_bump());
                    let fields = state.arena.alloc(fields);

                    PlutusData::map(state.arena, fields)
                }),
        ))
        .boxed()
    })
}
