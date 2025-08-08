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

use crate::{CborWrap, DatumHash, DatumOption, PlutusData};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BorrowedDatumOption<'a> {
    Hash(&'a DatumHash),
    Data(&'a CborWrap<PlutusData>),
}

impl<'a> From<&'a DatumOption> for BorrowedDatumOption<'a> {
    fn from(value: &'a DatumOption) -> Self {
        match value {
            DatumOption::Hash(hash) => Self::Hash(hash),
            DatumOption::Data(cbor_wrap) => Self::Data(cbor_wrap),
        }
    }
}

// FIXME: we are cloning here ('to_owned'). Can we avoid that?
impl From<BorrowedDatumOption<'_>> for DatumOption {
    fn from(value: BorrowedDatumOption<'_>) -> Self {
        match value {
            BorrowedDatumOption::Hash(hash) => Self::Hash(*hash),
            BorrowedDatumOption::Data(cbor_wrap) => {
                Self::Data(CborWrap(cbor_wrap.to_owned().unwrap()))
            }
        }
    }
}
