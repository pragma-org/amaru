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

// This is a stub lib.rs.

/// Marker so dependents can reference this workspace-hack crate.
///
/// Hakari adds `amaru-deps` to unify features; it is never imported otherwise, which
/// trips `cargo::unused_dependencies`. Dependents should use this constant privately
/// (`const _: () = amaru_deps::AMARU_DEPS_USED`), not re-export it.
pub const AMARU_DEPS_USED: () = ();
