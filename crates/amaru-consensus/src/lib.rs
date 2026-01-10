// Copyright 2024 PRAGMA
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

pub use amaru_ouroboros_traits::*;

pub mod consensus;

#[cfg(test)]
pub(crate) mod test {
    macro_rules! include_header {
        ($name:ident, $slot:expr) => {
            static $name: std::sync::LazyLock<BlockHeader> = std::sync::LazyLock::new(|| {
                let data =
                    include_bytes!(concat!("../../tests/data/headers/preprod_", $slot, ".cbor"));
                amaru_kernel::from_cbor(data.as_slice()).expect("invalid header")
            });
        };
    }

    pub(crate) use include_header;
}
