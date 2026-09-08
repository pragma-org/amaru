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

mod debruijn;
mod name;
mod named_debruijn;

pub use debruijn::*;
pub use name::*;
pub use named_debruijn::*;

use crate::{arena::Arena, flat};

pub trait Binder<'a>: std::fmt::Debug {
    // this might not need to return a Result
    fn var_encode(&self, e: &mut flat::Encoder) -> Result<(), flat::FlatEncodeError>;
    fn var_decode(arena: &'a Arena, d: &mut flat::Decoder<'_>) -> Result<&'a Self, flat::FlatDecodeError>;

    // this might not need to return a Result
    fn parameter_encode(&self, e: &mut flat::Encoder) -> Result<(), flat::FlatEncodeError>;
    fn parameter_decode(arena: &'a Arena, d: &mut flat::Decoder<'_>) -> Result<&'a Self, flat::FlatDecodeError>;
}

pub trait Eval<'a>: Binder<'a> {
    fn index(&self) -> usize;
}
