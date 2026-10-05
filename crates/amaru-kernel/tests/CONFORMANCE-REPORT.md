# The decoding conformance report

`test_cbor_dataset` writes a JSON report of the run when `AMARU_TEST_REPORT_DIRECTORY` is set. CI sets it and uploads
the file as the `cbor-conformance-report` artifact, so a run can be read, diffed against an earlier one, or tracked
over time without re-running the suite.

```text
AMARU_TEST_REPORT_DIRECTORY=tests/results cargo test -p amaru-kernel --test test_cbor_dataset -- --nocapture
```

The file is named `amaru-decoding-conformance_<corpus>_<protocol_version>.json`, for example
`amaru-decoding-conformance_conway-123-100_10.0.json`. Its content is one pretty-printed JSON object.

## Top level

| key                | type                | meaning                                                             |
|--------------------|---------------------|---------------------------------------------------------------------|
| `corpus`           | string              | the dataset that was run, e.g. `conway-123-100`                      |
| `protocol_version` | string              | the protocol version decoded against, e.g. `10.0`                    |
| `successful`       | bool                | true when the run had no failure at all; acknowledgements do not make it true |
| `totals`           | outcome             | the per-rule outcomes summed                                         |
| `rules`            | map rule → outcome  | one outcome per CDDL rule, keyed by rule name, sorted                |
| `failures`         | array of failures   | one entry per failing sample                                         |

## Outcome

The shape used by `totals` and by every value of `rules`. Every field counts samples.

| field                                  | meaning                                                                         |
|----------------------------------------|---------------------------------------------------------------------------------|
| `generated_total`                      | samples generated from the CDDL for this rule                                    |
| `generated_decoded_reencoded_expected` | of those, the ones that must decode, re-encode, and match the reference bytes    |
| `generated_decoded_reencoded_actual`   | the ones that did                                                                |
| `generated_must_be_rejected_expected`  | generated samples that must be rejected, even though they satisfy the CDDL       |
| `generated_must_be_rejected_actual`    | the ones that were                                                               |
| `zap_must_be_rejected_expected`        | malformed mutations that must be rejected                                        |
| `zap_must_be_rejected_actual`          | the ones that were                                                               |

A rule is clean when each `_actual` equals its `_expected`. For every rule,
`generated_total` = `generated_decoded_reencoded_expected` + `generated_must_be_rejected_expected`.

## Failure

| field    | meaning                                                                                               |
|----------|--------------------------------------------------------------------------------------------------------|
| `sample` | `<rule>/<category>/<file stem>`, where `<category>` is `valid` or `invalid/zap-<n>` for severity `n`   |
| `rule`   | the CDDL rule the sample belongs to                                                                      |
| `class`  | `reason` collapsed into a stable label: first line only, byte offset dropped, long digit runs replaced by `N`, truncated at 96 characters |
| `reason` | the full error text, which may span several lines and embed hex dumps                                    |

```json
{
  "sample": "auxiliary_data/valid/00003-2f2bacfd41c291d4",
  "rule": "auxiliary_data",
  "class": "re-encoding differs from the cbor reference",
  "reason": "re-encoding differs from the cbor reference\n\nexpected\n\nd90103a400a11be2de…"
}
```

## Reading a report

`(rule, class)` is the grouping key of the whole report, and it is also exactly what an entry of
`acknowledged-failures.toml` names. A new `(rule, class)` pair is a new defect; a pair that disappears is progress.

`successful: false` does not fail the test run. The suite fails on failures that are *not* declared in
`acknowledged-failures.toml`, so the report describes conformance while the acknowledgement file is what gates CI.
