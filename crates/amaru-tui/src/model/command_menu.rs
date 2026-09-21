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

/// The active level of the keyboard command menu.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandMenu {
    /// Direct navigation and mode-switching commands.
    #[default]
    Default,
    /// Commands that operate on the log pane.
    Logs,
    /// Explicit confirmation before requesting shutdown.
    Quit,
}
