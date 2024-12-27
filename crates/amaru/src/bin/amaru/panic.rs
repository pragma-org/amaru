use owo_colors::OwoColorize;

/// Installs a panic handler that prints some useful diagnostics and
/// asks the user to report the issue.
pub fn panic_handler() {
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| {
                info.payload()
                    .downcast_ref::<String>()
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "unknown error".to_string());

        let location = info.location().map_or_else(
            || "".into(),
            |location| {
                format!(
                    "{}:{}:{}\n\n    ",
                    location.file(),
                    location.line(),
                    location.column(),
                )
            },
        );

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

                        {location}{message}"#,
            info = node_info(),
            fatal = "amaru::fatal::error".red().bold(),
            location = location.purple(),
        };

        println!("\n{}", indent(&error_message, 3));
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
    let diff = n as i32 - ansi_len(&text) as i32;
    if diff.is_positive() {
        for _ in 0..diff {
            text.insert_str(0, delimiter);
        }
    }
    text
}

pub fn ansi_len(s: &str) -> usize {
    String::from_utf8(strip_ansi_escapes::strip(s).unwrap())
        .unwrap()
        .chars()
        .count()
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
