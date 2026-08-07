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

use amaru_observability::info;

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

static PACKAGE_VERSION: LazyLock<String> = LazyLock::new(|| {
    let version = format!(
        "{}.{}.{}",
        built_info::PKG_VERSION_MAJOR,
        built_info::PKG_VERSION_MINOR,
        built_info::PKG_VERSION_PATCH,
    );

    if built_info::PKG_VERSION_PRE.is_empty() { version } else { format!("{version}-{}", built_info::PKG_VERSION_PRE) }
});

static DISPLAY_VERSION: LazyLock<String> = LazyLock::new(|| match (git_commit_hash_short(), git_dirty()) {
    (Some(sha), Some(true)) => format!("{} ({sha}+dirty)", package_version()),
    (Some(sha), _) => format!("{} ({sha})", package_version()),
    _ => package_version().to_string(),
});

pub fn package_version() -> &'static str {
    PACKAGE_VERSION.as_str()
}

pub fn display_version() -> &'static str {
    DISPLAY_VERSION.as_str()
}

pub fn git_commit_hash() -> Option<&'static str> {
    built_info::GIT_COMMIT_HASH
}

pub fn git_commit_hash_short() -> Option<&'static str> {
    built_info::GIT_COMMIT_HASH_SHORT
}

pub fn git_dirty() -> Option<bool> {
    built_info::GIT_DIRTY
}

pub fn target_os() -> &'static str {
    built_info::CFG_OS
}

pub fn target_arch() -> &'static str {
    built_info::CFG_TARGET_ARCH
}

/// Emit a structured INFO event with the running binary's version and git identity.
///
/// Call this once after the tracing subscriber is installed so operator log files
/// record which build produced them.
pub fn log_build_version() {
    info!(
        setup::build::VERSION,
        version = package_version(),
        git_commit = git_commit_hash().unwrap_or("unknown"),
        git_dirty = git_dirty().unwrap_or(false),
        os = target_os(),
        arch = target_arch(),
    );
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    /// Captures fmt layer output so tests can assert on emitted events.
    #[derive(Clone, Default)]
    struct CaptureWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        fn contents(&self) -> String {
            let bytes = self.buffer.lock().expect("capture buffer lock").clone();
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buffer.lock().expect("capture buffer lock").write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn log_build_version_emits_package_and_git_fields() {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            // Equivalent to with_test_writer for cargo test visibility, but captureable for asserts.
            .with_max_level(tracing::Level::INFO)
            .with_target(true)
            .with_level(true)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            log_build_version();
        });

        let output = writer.contents();

        assert!(
            output.contains("amaru::setup") && output.contains("build.version"),
            "expected amaru::setup build.version event target in output:\n{output}"
        );
        assert!(
            output.contains(package_version()),
            "expected package version {} in output:\n{output}",
            package_version()
        );
        assert!(output.contains(target_os()), "expected os {} in output:\n{output}", target_os());
        assert!(output.contains(target_arch()), "expected arch {} in output:\n{output}", target_arch());

        let expected_commit = git_commit_hash().unwrap_or("unknown");
        assert!(output.contains(expected_commit), "expected git commit {expected_commit} in output:\n{output}");
    }
}
