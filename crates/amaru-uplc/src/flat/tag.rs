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

// Widths
pub const TERM_TAG_WIDTH: usize = 4;
pub const CONST_TAG_WIDTH: usize = 4;
pub const BUILTIN_TAG_WIDTH: usize = 7;

// Term Tags
pub const VAR: u8 = 0;
pub const DELAY: u8 = 1;
pub const LAMBDA: u8 = 2;
pub const APPLY: u8 = 3;
pub const CONSTANT: u8 = 4;
pub const FORCE: u8 = 5;
pub const ERROR: u8 = 6;
pub const BUILTIN: u8 = 7;
pub const CONSTR: u8 = 8;
pub const CASE: u8 = 9;

// Constant Tags
pub const INTEGER: u8 = 0;
pub const BYTE_STRING: u8 = 1;
pub const STRING: u8 = 2;
pub const UNIT: u8 = 3;
pub const BOOL: u8 = 4;
pub const DATA: u8 = 8;
pub const BLS12_381_G1_ELEMENT: u8 = 9;
pub const BLS12_381_G2_ELEMENT: u8 = 10;
pub const BLS12_381_ML_RESULT: u8 = 11;
pub const VALUE: u8 = 13;
pub const PROTO_LIST_ONE: u8 = 7;
pub const PROTO_LIST_TWO: u8 = 5;
pub const PROTO_ARRAY_ONE: u8 = 7;
pub const PROTO_ARRAY_TWO: u8 = 12;
pub const PROTO_PAIR_ONE: u8 = 7;
pub const PROTO_PAIR_TWO: u8 = 7;
pub const PROTO_PAIR_THREE: u8 = 6;
