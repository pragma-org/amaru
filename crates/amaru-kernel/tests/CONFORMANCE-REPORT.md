# The decoding conformance report

`test_cbor_dataset` writes a JSON report of the run when `AMARU_TEST_REPORT_DIRECTORY` is set. CI sets it and uploads
the file as the `cbor-conformance-report` artifact, so a run can be read, diffed against an earlier one, or tracked
over time without re-running the suite.

```text
AMARU_TEST_REPORT_DIRECTORY=tests/results cargo test -p amaru-kernel --test test_cbor_dataset -- --nocapture
```

The file is named `amaru-latest.json`, the name the corpus gives an implementation's own report, and the site that
aggregates reports across implementations reads it under that name.

## Shape

The report is a structured summary rather than a per-sample map. The sample-level failures remain available in the test
output, while the published report contains the counters needed to compare implementations.

| field | meaning |
|-------|---------|
| `version` | Report schema version, currently `"1"`. |
| `scenario` | Fixed value `"cbor-decoding"`. |
| `generated_at` | Unix timestamp in milliseconds. |
| `implementation.name` | Implementation that produced the report. |
| `corpus` | Corpus identifier used for the run. Reports from different corpus versions are not directly comparable. |
| `protocol_version` | Protocol version used for decoding and re-encoding. |
| `successful` | `true` when no sample failures were observed. Acknowledged failures remain reflected here. |
| `totals` | Aggregate [`TestOutcome`](common/test_outcome.rs) counters for the complete run. |
| `rules` | One `TestOutcome` object per CDDL rule. |

Each `TestOutcome` contains these counters:

- `generated_total`
- `generated_decoded_reencoded_expected` and `generated_decoded_reencoded_actual`
- `generated_must_be_rejected_expected` and `generated_must_be_rejected_actual`
- `zap_must_be_rejected_expected` and `zap_must_be_rejected_actual`

The node-diversity-conformance aggregator sums the `*_expected` and `*_actual` counters to compute the conformance score.

```json
{
  "version": "1",
  "scenario": "cbor-decoding",
  "generated_at": 1792344981442,
  "implementation": { "name": "amaru" },
  "corpus": "conway-123-100",
  "protocol_version": "10.0",
  "successful": true,
  "totals": {
    "generated_total": 2500,
    "generated_decoded_reencoded_expected": 866,
    "generated_decoded_reencoded_actual": 866,
    "generated_must_be_rejected_expected": 1634,
    "generated_must_be_rejected_actual": 1634,
    "zap_must_be_rejected_expected": 6700,
    "zap_must_be_rejected_actual": 6700
  },
  "rules": {
    "block": {
      "generated_total": 100,
      "generated_decoded_reencoded_expected": 0,
      "generated_decoded_reencoded_actual": 0,
      "generated_must_be_rejected_expected": 100,
      "generated_must_be_rejected_actual": 100,
      "zap_must_be_rejected_expected": 300,
      "zap_must_be_rejected_actual": 300
    }
  }
}
```
