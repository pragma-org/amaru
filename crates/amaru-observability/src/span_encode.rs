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

//! Compact span-path encoding for human-facing sinks (EDR-033).

use std::fmt::{self, Write};

/// Write a Java-style abbreviation of a span name: `epoch.transition` → `e.t`.
pub fn write_abbreviated_span_name(out: &mut impl Write, name: &str) -> fmt::Result {
    let mut first = true;
    for segment in name.split('.').filter(|segment| !segment.is_empty()) {
        if !first {
            out.write_char('.')?;
        }
        first = false;
        if let Some(ch) = segment.chars().next() {
            out.write_char(ch)?;
        }
    }
    Ok(())
}

/// Write ancestor names as an abbreviated path: `e.t:g.r`.
pub fn write_abbreviated_span_path<I, S>(out: &mut impl Write, names: I) -> fmt::Result
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut first = true;
    for name in names {
        if !first {
            out.write_char(':')?;
        }
        first = false;
        write_abbreviated_span_name(out, name.as_ref())?;
    }
    Ok(())
}

/// Abbreviate a tracing span name like a Java logger: `epoch.transition` → `e.t`.
///
/// Each `.`-separated segment keeps only its first character. A typical span
/// level then costs four characters on the console once the `:` separator is
/// included (`e.t:`).
pub fn abbreviate_span_name(name: &str) -> String {
    let mut out = String::new();
    let _ = write_abbreviated_span_name(&mut out, name);
    out
}

/// Ancestor names from a root-to-leaf list, excluding the wrapping (leaf) span.
pub fn ancestor_span_names<'a, I, S>(names_from_root: I) -> Box<dyn Iterator<Item = S> + 'a>
where
    I: IntoIterator<Item = S> + 'a,
{
    let mut iter = names_from_root.into_iter().peekable();
    Box::new(std::iter::from_fn(move || {
        let ret = iter.next();
        if iter.peek().is_none() { None } else { ret }
    }))
}

/// Join span names as an abbreviated path: `epoch.transition` / `governance.ratify_proposals`
/// → `e.t:g.r`.
pub fn format_abbreviated_span_path<I, S>(names: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = String::new();
    let _ = write_abbreviated_span_path(&mut out, names);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviates_dotted_span_names() {
        assert_eq!(abbreviate_span_name("epoch.transition"), "e.t");
        assert_eq!(abbreviate_span_name("governance.ratify_proposals"), "g.r");
        assert_eq!(abbreviate_span_name("lifecycle"), "l");
        assert_eq!(abbreviate_span_name(""), "");
    }

    #[test]
    fn joins_abbreviated_levels_with_colons() {
        assert_eq!(format_abbreviated_span_path(["epoch.transition", "governance.ratify_proposals"]), "e.t:g.r");
        assert_eq!(format_abbreviated_span_path(["epoch.transition"]), "e.t");
        assert_eq!(format_abbreviated_span_path(Vec::<&str>::new()), "");
    }

    #[test]
    fn ancestor_list_drops_the_wrapping_span() {
        assert_eq!(
            ancestor_span_names(vec!["epoch.transition", "governance.ratify_proposals", "ratification.round"])
                .collect::<Vec<_>>(),
            vec!["epoch.transition", "governance.ratify_proposals"]
        );
        assert_eq!(ancestor_span_names(vec!["epoch.transition"]).collect::<Vec<_>>(), Vec::<&str>::new());
        assert_eq!(ancestor_span_names(Vec::<&str>::new()).collect::<Vec<_>>(), Vec::<&str>::new());
    }
}
