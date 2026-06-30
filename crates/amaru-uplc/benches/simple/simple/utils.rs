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

use amaru_uplc::{arena::Arena, binder::DeBruijn, machine::MachineVersion, program::Program, term::Term};
use ouroboros::self_referencing;

#[self_referencing]
pub struct BenchState {
    pub arena: Arena,
    #[borrows(arena)]
    #[covariant]
    pub program: &'this Program<'this, DeBruijn>,
}

impl BenchState {
    #[inline]
    pub fn exec(&self) {
        self.with_program(|program| {
            self.with_arena(|arena| {
                let _ = program.eval_default(arena);
            });
        });
    }
}

pub fn setup_program<F>(program_builder: F) -> BenchState
where
    F: for<'this> FnOnce(&'this Arena) -> &'this Program<'this, DeBruijn>,
{
    let arena = Arena::new();

    let builder = BenchStateBuilder { arena, program_builder };

    builder.build()
}

#[inline]
pub fn setup_term<F>(term_builder: F) -> BenchState
where
    F: for<'this> FnOnce(&'this Arena) -> &'this Term<'this, DeBruijn>,
{
    setup_program(|arena| {
        let term = term_builder(arena);

        let version = MachineVersion::V1_1_0;

        Program::new(arena, version, term)
    })
}
