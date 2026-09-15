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

use std::collections::BTreeMap;

use amaru_kernel::NonEmptyVec;

use crate::{TestConfiguration, TestResults};

pub fn report(test_configuration: &TestConfiguration, test_results: &TestResults) -> anyhow::Result<()> {
    report_json(test_configuration, test_results)?;
    report_console(test_configuration, test_results);
    Ok(())
}

fn report_console(test_configuration: &TestConfiguration, test_results: &TestResults) {
    if test_results.failures.is_empty() {
        return;
    }
    print_failure_causes(&test_configuration, &test_results);
    print_summary(&test_configuration, &test_results);

    if !test_results.is_successful() {
        let total = test_results.total();
        panic!(
            "{} of {total} samples did not match their expectation; see the summary above",
            test_results.failures.len()
        );
    }
}

fn print_failure_causes(test_configuration: &TestConfiguration, test_results: &TestResults) {
    let mut causes = BTreeMap::new();
    for (key, reason) in &test_results.failures {
        let class = error_class(reason);
        causes.entry(class).or_insert_with(|| (reason.clone(), NonEmptyVec::singleton(key.clone())));
    }

    eprintln!();
    eprintln!("----------------------------------------------------------------------------");
    eprintln!(
        "FAILURE CAUSES for cbor dataset {} (protocol version {})",
        test_configuration.corpus(),
        test_configuration.protocol_version()
    );
    eprintln!("----------------------------------------------------------------------------");
    eprintln!();
    for (class, (reason, keys)) in causes {
        let rule = keys.first().rule();
        let t = test_results.get_test_outcome(rule);
        let count = keys.len();
        eprintln!("{count:>3} samples failed with class '{class}'");
        eprintln!("  example sample: {}", keys.first());
        eprintln!("  reason: {reason}");
        eprintln!(
            "rule summary:\n  decoded and re-encoded ok {}/{}\n  generated rejected ok     {}/{}\n  zapped rejected ok        {}/{}",
            t.generated_decoded_reencoded_actual,
            t.generated_decoded_reencoded_expected,
            t.generated_must_be_rejected_actual,
            t.generated_must_be_rejected_expected,
            t.zap_must_be_rejected_actual,
            t.zap_must_be_rejected_expected
        );
        eprintln!();
    }
}

fn print_summary(test_configuration: &TestConfiguration, test_results: &TestResults) {
    let totals = test_results.totals();
    let generated_total = totals.generated_total;
    let generated_decoded_reencoded_actual = totals.generated_decoded_reencoded_actual;
    let generated_decoded_reencoded_expected = totals.generated_decoded_reencoded_expected;
    let check_generated_decoded_reencoded =
        check(generated_decoded_reencoded_actual, generated_decoded_reencoded_expected);
    let generated_must_be_rejected_actual = totals.generated_must_be_rejected_actual;
    let generated_must_be_rejected_expected = totals.generated_must_be_rejected_expected;
    let check_generated_must_be_rejected =
        check(generated_must_be_rejected_actual, generated_must_be_rejected_expected);
    let zap_must_be_rejected_actual = totals.zap_must_be_rejected_actual;
    let zap_must_be_rejected_expected = totals.zap_must_be_rejected_expected;
    let check_zap_must_be_rejected = check(zap_must_be_rejected_actual, zap_must_be_rejected_expected);

    eprintln!();
    eprintln!("----------------------------------------------------------------------------");
    eprintln!(
        "SUMMARY for cbor dataset {} (protocol version {})",
        test_configuration.corpus(),
        test_configuration.protocol_version()
    );
    eprintln!("----------------------------------------------------------------------------");
    eprintln!();
    eprintln!("generated samples           {generated_total} ");
    eprintln!();
    eprintln!(
        "generated samples decoded   {generated_decoded_reencoded_actual}/{generated_decoded_reencoded_expected} {check_generated_decoded_reencoded}"
    );
    eprintln!("re-encoded as expected");
    eprintln!();
    eprintln!(
        "generated samples rejected  {generated_must_be_rejected_actual}/{generated_must_be_rejected_expected} {check_generated_must_be_rejected}"
    );
    eprintln!("as expected");
    eprintln!();
    eprintln!(
        "malformed samples rejected  {zap_must_be_rejected_actual}/{zap_must_be_rejected_expected} {check_zap_must_be_rejected}"
    );
    eprintln!("as expected ");
    eprintln!();
    eprintln!("----------------------------------------------------------------------------");
    eprintln!("BY RULE ");
    eprintln!("----------------------------------------------------------------------------");
    eprintln!();
    eprintln!("  rule                          decoded and      generated         zapped");
    eprintln!("                               re-encoded ok    rejected ok      rejected ok");
    eprintln!();
    for (rule, t) in &test_results.per_rule {
        let check_decoded_reencoded =
            check(t.generated_decoded_reencoded_actual, t.generated_decoded_reencoded_expected);
        let check_generated_rejected =
            check(t.generated_must_be_rejected_actual, t.generated_must_be_rejected_expected);
        let check_zapped_rejected = check(t.zap_must_be_rejected_actual, t.zap_must_be_rejected_expected);
        eprintln!(
            "{rule:<30} {:>3}/{:<3} {}        {:>3}/{:<3} {}      {:>3}/{:<3} {}",
            t.generated_decoded_reencoded_actual,
            t.generated_decoded_reencoded_expected,
            check_decoded_reencoded,
            t.generated_must_be_rejected_actual,
            t.generated_must_be_rejected_expected,
            check_generated_rejected,
            t.zap_must_be_rejected_actual,
            t.zap_must_be_rejected_expected,
            check_zapped_rejected
        );
    }
}

/// Write the run's counters and failures as JSON, so another tool can diff runs or track progress.
fn report_json(test_configuration: &TestConfiguration, test_results: &TestResults) -> anyhow::Result<()> {
    let Some(directory) = test_configuration.report_directory() else { return Ok(()) };

    let failures = test_results
        .failures
        .iter()
        .map(|(key, reason)| {
            serde_json::json!({
                "sample": key.key(),
                "rule": key.rule(),
                "class": error_class(reason),
                "reason": reason,
            })
        })
        .collect::<Vec<_>>();

    let report = serde_json::json!({
        "corpus": test_configuration.corpus().to_string(),
        "protocol_version": test_configuration.protocol_version().to_string(),
        "successful": test_results.is_successful(),
        "totals": test_results.totals(),
        "rules": test_results.per_rule,
        "failures": failures,
    });

    std::fs::create_dir_all(directory)?;
    let file_name = format!(
        "amaru-decoding-conformance_{}_{}.json",
        test_configuration.corpus(),
        test_configuration.protocol_version()
    );
    let file = directory.join(file_name);
    std::fs::write(file, serde_json::to_string_pretty(&report)?)?;
    Ok(())
}

fn check(actual: usize, expected: usize) -> &'static str {
    if actual == expected { "✅" } else { "❌" }
}

/// Collapse a decode error into a class label so identical defects group together.
///
/// The byte offset is dropped, keeping whatever detail follows it, and long literal values are
/// replaced by `N`. Short digit runs survive so that type widths such as `u64` stay legible. The
/// result is truncated, since some messages embed whole hex dumps that would otherwise swamp the
/// summary.
fn error_class(reason: &str) -> String {
    const MAX_WIDTH: usize = 96;
    /// Digit runs at least this long are values (offsets, thresholds), not type widths.
    const LITERAL_DIGITS: usize = 4;
    const AT_POSITION: &str = " at position ";

    let without_offset = match reason.find(AT_POSITION) {
        Some(start) => {
            let after = &reason[start + AT_POSITION.len()..];
            let detail = after.find(": ").map(|i| &after[i..]).unwrap_or("");
            format!("{}{}", &reason[..start], detail)
        }
        None => reason.to_string(),
    };

    let mut out = String::with_capacity(without_offset.len());
    let mut digits = String::new();
    for c in without_offset.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if digits.len() >= LITERAL_DIGITS {
            out.push('N');
        } else {
            out.push_str(&digits);
        }
        digits.clear();
        out.push(c);
    }
    if digits.len() >= LITERAL_DIGITS {
        out.push('N');
    } else {
        out.push_str(&digits);
    }

    if out.chars().count() > MAX_WIDTH {
        out = out.chars().take(MAX_WIDTH).chain(std::iter::once('…')).collect();
    }
    out
}
