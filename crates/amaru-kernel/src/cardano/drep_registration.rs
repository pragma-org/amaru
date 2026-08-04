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

use crate::{CertificatePointer, DRepState, Epoch, Lovelace};

#[derive(Debug, Copy, Clone)]
pub struct DRepRegistration {
    pub deposit: Lovelace,
    pub registered_at: CertificatePointer,
    pub valid_until: Epoch,
}

impl DRepRegistration {
    /// Construct a new `DRepRegistration` from a decoded state and a registration pointer.
    pub fn from_state(state: DRepState, registered_at: CertificatePointer) -> DRepRegistration {
        DRepRegistration { deposit: state.deposit, registered_at, valid_until: state.expiry }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::DRepRegistration;
    use crate::{any_certificate_pointer, any_epoch, any_lovelace};

    prop_compose! {
        pub fn any_drep_registration()(
            deposit in any_lovelace() ,
            registered_at in any_certificate_pointer(u64::MAX),
            valid_until in any_epoch(),
        ) -> DRepRegistration {
            DRepRegistration { deposit, registered_at, valid_until }
        }
    }
}
