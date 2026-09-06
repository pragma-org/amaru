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

use super::{Binder, Eval};
use crate::arena::Arena;

#[derive(Debug)]
pub struct NamedDeBruijn<'a> {
    text: &'a str,
    index: usize,
}

impl<'a> NamedDeBruijn<'a> {
    pub fn new(arena: &'a Arena, text: &'a str, index: usize) -> &'a Self {
        arena.alloc(NamedDeBruijn { text, index })
    }
}

impl<'a> Binder<'a> for NamedDeBruijn<'a> {
    fn var_encode(&self, e: &mut crate::flat::Encoder) -> Result<(), crate::flat::FlatEncodeError> {
        e.utf8(self.text)?;
        e.word(self.index);

        Ok(())
    }

    fn var_decode(
        arena: &'a Arena,
        d: &mut crate::flat::Decoder<'_>,
    ) -> Result<&'a Self, crate::flat::FlatDecodeError> {
        let text = d.utf8(arena)?;
        let index = d.word()?;

        let nd = NamedDeBruijn::new(arena, text, index);

        Ok(nd)
    }

    fn parameter_encode(&self, e: &mut crate::flat::Encoder) -> Result<(), crate::flat::FlatEncodeError> {
        self.var_encode(e)
    }

    fn parameter_decode(
        arena: &'a Arena,
        d: &mut crate::flat::Decoder<'_>,
    ) -> Result<&'a Self, crate::flat::FlatDecodeError> {
        Self::var_decode(arena, d)
    }
}

impl<'a> Eval<'a> for NamedDeBruijn<'a> {
    fn index(&self) -> usize {
        self.index
    }
}
