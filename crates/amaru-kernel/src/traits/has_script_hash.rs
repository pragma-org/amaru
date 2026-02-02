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

use crate::{
    Hash, Hasher, KeepRaw, MemoizedNativeScript, MemoizedScript, NativeScript, PlutusScript,
    size::SCRIPT,
};

pub trait HasScriptHash {
    /*
        To compute a script hash, one must prepend a tag to the bytes of the script before hashing.

        The tag (u8) is determined by the language, as such:

          0 for native scripts
          1 for Plutus V1 scripts
          2 for Plutus V2 scripts
          3 for Plutus V3 scripts
    */
    fn script_hash(&self) -> Hash<SCRIPT>;
}

// ----------------------------------------------------------------------------
// Implementations
// ----------------------------------------------------------------------------

impl<const VERSION: usize> HasScriptHash for PlutusScript<VERSION> {
    fn script_hash(&self) -> Hash<SCRIPT> {
        tagged_script_hash(VERSION as u8, self.as_ref())
    }
}

impl HasScriptHash for MemoizedScript {
    fn script_hash(&self) -> Hash<SCRIPT> {
        match self {
            Self::NativeScript(native_script) => native_script.script_hash(),
            Self::PlutusV1Script(plutus_script) => plutus_script.script_hash(),
            Self::PlutusV2Script(plutus_script) => plutus_script.script_hash(),
            Self::PlutusV3Script(plutus_script) => plutus_script.script_hash(),
        }
    }
}

impl HasScriptHash for MemoizedNativeScript {
    fn script_hash(&self) -> Hash<SCRIPT> {
        native_script_hash(self.original_bytes())
    }
}

impl HasScriptHash for KeepRaw<'_, NativeScript> {
    fn script_hash(&self) -> Hash<SCRIPT> {
        native_script_hash(self.raw_cbor())
    }
}

// ----------------------------------------------------------------------------
// Internals
// ----------------------------------------------------------------------------

fn native_script_hash(bytes: &[u8]) -> Hash<SCRIPT> {
    tagged_script_hash(0, bytes)
}

fn tagged_script_hash(tag: u8, bytes: &[u8]) -> Hash<SCRIPT> {
    Hasher::<{ 8 * SCRIPT }>::hash_tagged(bytes, tag)
}
