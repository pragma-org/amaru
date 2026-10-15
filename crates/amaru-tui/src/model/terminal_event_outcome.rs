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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEventOutcome {
    Continue,
    EnterCopyMode,
    ExitCopyMode,
    ExportLogs,
    Shutdown,
    /// The pointer moved on a text row. The session freezes the frame it has already painted.
    StartTextDrag,
    UpdateTextDrag,
    /// Mouse-up after a text drag. Coordinates are the press and the release.
    FinishTextDrag {
        anchor: (u16, u16),
        current: (u16, u16),
    },
    /// Double-click. `cells` are the visible columns of the value.
    CopyText {
        text: String,
        cells: Vec<(u16, u16)>,
    },
}
