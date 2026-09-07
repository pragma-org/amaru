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

#[derive(Debug, Eq, PartialEq)]
pub struct DeBruijn(usize);

impl DeBruijn {
    pub fn new(arena: &Arena, i: usize) -> &Self {
        arena.alloc(DeBruijn(i))
    }

    pub fn zero(arena: &Arena) -> &Self {
        arena.alloc(DeBruijn(0))
    }
}

impl<'a> Binder<'a> for DeBruijn {
    fn var_encode(&self, e: &mut crate::flat::Encoder) -> Result<(), crate::flat::FlatEncodeError> {
        e.word(self.0);

        Ok(())
    }

    fn var_decode(
        arena: &'a Arena,
        d: &mut crate::flat::Decoder<'_>,
    ) -> Result<&'a Self, crate::flat::FlatDecodeError> {
        let i = d.word()?;

        let d = DeBruijn::new(arena, i);

        Ok(d)
    }

    fn parameter_encode(&self, _e: &mut crate::flat::Encoder) -> Result<(), crate::flat::FlatEncodeError> {
        Ok(())
    }

    fn parameter_decode(
        arena: &'a Arena,
        _d: &mut crate::flat::Decoder<'_>,
    ) -> Result<&'a Self, crate::flat::FlatDecodeError> {
        let d = DeBruijn::new(arena, 0);

        Ok(d)
    }
}

impl Eval<'_> for DeBruijn {
    fn index(&self) -> usize {
        self.0
    }
}
