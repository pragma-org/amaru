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

#[macro_export]
macro_rules! constr {
    ($to_plutus_data_ann:path, $index:expr, [$($field:expr),* $(,)?]) => {{
        let maybe_constr_tag = $crate::to_cbor_tag($index);
        amaru_kernel::PlutusData::Constr(amaru_kernel::Constr {
            tag: maybe_constr_tag.unwrap_or(102),
            any_constructor: maybe_constr_tag.map_or(Some($index), |_| None),
            fields: amaru_kernel::MaybeIndefArray::Indef(vec![$($to_plutus_data_ann(&$field)),*]),
        })
    }};

    ($index:expr, [$($field:expr),* $(,)?] $(,)?) => {{
        constr!($crate::ToPlutusData::to_plutus_data, $index, [$($field),*])
    }};

    ($index:expr $(,)?) => {{
        constr!(ToPlutusData, $index, [])
    }};
}

#[macro_export]
macro_rules! constr_v1 {
    ($index:expr, [$($field:expr),* $(,)?] $(,)?) => {{ $crate::constr!($crate::ToPlutusData::<1>::to_plutus_data, $index, [$($field),*]) }};
}

#[macro_export]
macro_rules! constr_v2 {
    ($index:expr, [$($field:expr),* $(,)?] $(,)?) => {{ $crate::constr!($crate::ToPlutusData::<2>::to_plutus_data, $index, [$($field),*]) }};
}

#[macro_export]
macro_rules! constr_v3 {
    ($index:expr, [$($field:expr),* $(,)?] $(,)?) => {{ $crate::constr!($crate::ToPlutusData::<3>::to_plutus_data, $index, [$($field),*]) }};
}
