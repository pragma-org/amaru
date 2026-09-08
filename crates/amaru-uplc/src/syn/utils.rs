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

use chumsky::prelude::*;

use super::types::Extra;

// This is based on the grammar definition of `Name` from the Plutus Core Spec (https://plutus.cardano.intersectmbo.org/resources/plutus-core-spec.pdf 2.1.1)
// Name 𝑛 ∶∶= [a-zA-Z][a-zA-Z0-9_']
pub fn name<'a>() -> impl Parser<'a, &'a str, &'a str, Extra<'a>> {
    any()
        .filter(|c: &char| c.is_ascii_alphabetic())
        .then(any().filter(|c: &char| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '\'').repeated())
        .to_slice()
}

pub fn hex_digit<'a>() -> impl Parser<'a, &'a str, u8, Extra<'a>> {
    #[expect(clippy::unwrap_used)]
    one_of("0123456789abcdefABCDEF").map(|c: char| c.to_digit(16).unwrap() as u8)
}

pub fn hex_bytes<'a>() -> impl Parser<'a, &'a str, Vec<u8>, Extra<'a>> {
    hex_digit().then(hex_digit()).map(|(high, low)| (high << 4) | low).repeated().collect()
}

pub fn comments<'a>() -> impl Parser<'a, &'a str, (), Extra<'a>> {
    just("--").then(any().and_is(just('\n').not()).repeated()).padded().repeated()
}
