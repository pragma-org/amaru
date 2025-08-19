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

use std::process::exit;

/// Installs a panic handler that prints some useful diagnostics and
/// asks the user to report the issue.
pub fn panic_handler() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // We present the user with a helpful and welcoming error message;
        // Block producing nodes should be considered mission critical software, and so
        // They should endeavor *never* to crash, and should always handle and recover from errors.
        // So if the process panics, it should be treated as a bug, and we very much want the user to report it.
        // TODO(pi): We could go a step further, and prefill some issue details like title, body, labels, etc.
        //           using query parameters: https://github.com/sindresorhus/new-github-issue-url?tab=readme-ov-file#api
        let error_message = indoc::formatdoc! {
            r#"{fatal}
                        Whoops! The Amaru process panicked, rather than handling the error it encountered gracefully.

                        This is almost certainly a bug, and we'd appreciate a report so we can improve Amaru.

                        Please report this error at https://github.com/pragma-org/amaru/issues/new.

                        In your bug report please provide the information below and if possible the code
                        that produced it.
                        {info}
                        
                        "#,
            info = node_info(),
            fatal = "amaru::fatal::error",
        };
        eprintln!("\n{}", indent(&error_message, 3));
        prev(info);
        // Exit with a non-zero code to indicate that the process crashed
        // otherwise it will just sit there and wait for ctrl-c without saying so.
        exit(1);
    }));
}

// TODO: pulled from aiken; should we have our own utility crate for pretty printing?
// https://github.com/aiken-lang/aiken/blob/main/crates/aiken-project/src/pretty.rs#L126C1-L134C2
pub fn indent(lines: &str, n: usize) -> String {
    let tab = pad_left(String::new(), n, " ");
    lines
        .lines()
        .map(|line| format!("{tab}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn pad_left(mut text: String, n: usize, delimiter: &str) -> String {
    let diff = n as i32 - text.len() as i32;
    if diff.is_positive() {
        for _ in 0..diff {
            text.insert_str(0, delimiter);
        }
    }
    text
}

// TODO: pulled from aiken; should we have our own config utility crate?
// https://github.com/aiken-lang/aiken/blob/main/crates/aiken-project/src/config.rs#L382C1-L393C2
mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

pub fn node_info() -> String {
    format!(
        r#"
Operating System: {}
Architecture:     {}
Version:          {}"#,
        built_info::CFG_OS,
        built_info::CFG_TARGET_ARCH,
        node_version(true),
    )
}

pub fn node_version(include_commit_hash: bool) -> String {
    let version = built_info::PKG_VERSION;
    let suffix = if include_commit_hash {
        format!(
            "+{}",
            built_info::GIT_COMMIT_HASH_SHORT.unwrap_or("unknown")
        )
    } else {
        "".to_string()
    };
    format!("v{version}{suffix}")
}
