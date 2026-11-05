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

use std::{fs, io, path::Path};

use zeroize::Zeroizing;

#[cfg(all(not(target_family = "wasm"), not(target_arch = "riscv32")))]
const _: () = amaru_deps::AMARU_DEPS_USED;

pub mod credentials;
pub mod kes;
pub mod mempool;
pub mod praos;
mod serde_util;
pub mod vrf;

pub use amaru_kernel::utils::envelope::ReadEnvelopeError;
pub use amaru_ouroboros_traits::*;
pub use credentials::{
    ensure_operational_certificate_accepted, forging_credentials_from_files, load_cold_verification_key,
    load_operational_certificate,
};
pub use mempool::*;

/// Read an exact-size raw key, otherwise use the envelope reader. File contents are zeroed on drop.
fn read_key_file<const SIZE: usize, T, E: From<io::Error>>(
    path: &Path,
    from_raw: impl FnOnce(&[u8; SIZE]) -> Result<T, E>,
    from_envelope: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let bytes = Zeroizing::new(fs::read(path)?);
    if let Ok(raw) = <&[u8; SIZE]>::try_from(bytes.as_slice()) {
        return from_raw(raw);
    }
    drop(bytes);
    from_envelope()
}
