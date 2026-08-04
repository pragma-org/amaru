// Copyright 2026 PRAGMA
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

use std::sync::LazyLock;

use bech32::{self, Bech32};

#[expect(clippy::unwrap_used, reason = "safe hard-coded Human-readable part")]
pub static HRP_ADDR: LazyLock<Hrp> = LazyLock::new(|| Hrp::parse("addr").unwrap());

#[expect(clippy::unwrap_used, reason = "safe hard-coded Human-readable part")]
pub static HRP_ADDR_TEST: LazyLock<Hrp> = LazyLock::new(|| Hrp::parse("addr_test").unwrap());

#[expect(clippy::unwrap_used, reason = "safe hard-coded Human-readable part")]
pub static HRP_ADDR_VKH: LazyLock<Hrp> = LazyLock::new(|| Hrp::parse("addr_vkh").unwrap());

#[expect(clippy::unwrap_used, reason = "safe hard-coded Human-readable part")]
pub static HRP_ADDR_SHARED_VKH: LazyLock<Hrp> = LazyLock::new(|| Hrp::parse("addr_shared_vkh").unwrap());

#[expect(clippy::unwrap_used, reason = "safe hard-coded Human-readable part")]
pub static HRP_STAKE: LazyLock<Hrp> = LazyLock::new(|| Hrp::parse("stake").unwrap());

#[expect(clippy::unwrap_used, reason = "safe hard-coded Human-readable part")]
pub static HRP_STAKE_TEST: LazyLock<Hrp> = LazyLock::new(|| Hrp::parse("stake_test").unwrap());

#[expect(clippy::unwrap_used, reason = "safe hard-coded Human-readable part")]
pub static HRP_STAKE_VKH: LazyLock<Hrp> = LazyLock::new(|| Hrp::parse("stake_vkh").unwrap());

#[expect(clippy::unwrap_used, reason = "safe hard-coded Human-readable part")]
pub static HRP_STAKE_SHARED_VKH: LazyLock<Hrp> = LazyLock::new(|| Hrp::parse("stake_shared_vkh").unwrap());

pub use bech32::{DecodeError, Hrp};

pub fn encode<T: AsRef<[u8]>>(hrp: Hrp, payload: T) -> Option<String> {
    bech32::encode::<Bech32>(hrp, payload.as_ref()).ok()
}

pub fn decode(s: &str) -> Result<(Hrp, Vec<u8>), bech32::DecodeError> {
    bech32::decode(s)
}
