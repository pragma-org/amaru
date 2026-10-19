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

//! Copy dashboard text onto the clipboard the terminal can actually reach.
//!
//! `pbcopy` covers a Mac that is running the node. OSC 52 covers a terminal
//! that honors it, including Zed. Inside tmux the paste buffer is the copy
//! Apple Terminal can use over SSH, because that terminal does not apply OSC 52.

use std::{
    io::Write,
    process::{Command, Stdio},
};

pub(crate) struct CopyAttempt {
    pub osc52: Vec<u8>,
    pub pbcopy: bool,
    pub tmux: bool,
}

pub(crate) fn prepare_copy(text: &str) -> CopyAttempt {
    CopyAttempt {
        osc52: osc52(text),
        pbcopy: write_stdin("pbcopy", &[], text),
        tmux: std::env::var_os("TMUX").is_some() && write_stdin("tmux", &["load-buffer", "-"], text),
    }
}

pub(crate) fn copy_status(pbcopy: bool, tmux: bool, osc_written: bool) -> &'static str {
    if pbcopy || (osc_written && !tmux) {
        "copied"
    } else if tmux {
        "copied to tmux buffer"
    } else {
        "copy failed"
    }
}

pub(crate) fn osc52(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len().saturating_mul(2).saturating_add(8));
    out.extend_from_slice(b"\x1b]52;c;");
    base64_encode(text.as_bytes(), &mut out);
    out.push(0x07);
    out
}

fn write_stdin(program: &str, args: &[&str], text: &str) -> bool {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };
    if stdin.write_all(text.as_bytes()).is_err() {
        drop(stdin);
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    drop(stdin);
    child.wait().is_ok_and(|status| status.success())
}

fn base64_encode(data: &[u8], out: &mut Vec<u8>) {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut index = 0;
    while index + 3 <= data.len() {
        let word = u32::from(data[index]) << 16 | u32::from(data[index + 1]) << 8 | u32::from(data[index + 2]);
        out.push(TABLE[((word >> 18) & 0x3f) as usize]);
        out.push(TABLE[((word >> 12) & 0x3f) as usize]);
        out.push(TABLE[((word >> 6) & 0x3f) as usize]);
        out.push(TABLE[(word & 0x3f) as usize]);
        index += 3;
    }
    match data.len() - index {
        1 => {
            let word = u32::from(data[index]) << 16;
            out.push(TABLE[((word >> 18) & 0x3f) as usize]);
            out.push(TABLE[((word >> 12) & 0x3f) as usize]);
            out.extend_from_slice(b"==");
        }
        2 => {
            let word = u32::from(data[index]) << 16 | u32::from(data[index + 1]) << 8;
            out.push(TABLE[((word >> 18) & 0x3f) as usize]);
            out.push(TABLE[((word >> 12) & 0x3f) as usize]);
            out.push(TABLE[((word >> 6) & 0x3f) as usize]);
            out.push(b'=');
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_is_the_clipboard_sequence() {
        assert_eq!(osc52("hi"), b"\x1b]52;c;aGk=\x07");
        assert_eq!(osc52("h"), b"\x1b]52;c;aA==\x07");
    }

    #[test]
    fn status_names_the_route_that_worked() {
        assert_eq!(copy_status(true, true, true), "copied");
        assert_eq!(copy_status(false, true, true), "copied to tmux buffer");
        assert_eq!(copy_status(false, false, true), "copied");
        assert_eq!(copy_status(false, false, false), "copy failed");
    }
}
