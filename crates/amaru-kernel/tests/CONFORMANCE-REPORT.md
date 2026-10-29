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

The report uses the format described in [cardano-cbor-dataset](https://github.com/r2rationality/cardano-cbor-dataset).

| field                   | meaning                                                                             |
|-------------------------|-------------------------------------------------------------------------------------|
| `version`               | Report format version                                                               |
| `generated_at`          | Unix timestamp in milliseconds                                                     |
| `protocol_version`      | Protocol version used for decoding and re-encoding, as `<major>.<minor>`           |
| `dataset_commit`        | The dataset commit that holds the samples, read from `tests/data/cbor.dataset.pin` |
| `implementation_commit` | The Amaru commit that ran the samples: `git rev-parse HEAD`                        |
| `results`               | The outcome of every sample that ran                                               |

The keys of `results` are the sample paths relative to the corpus root, with forward slashes and the `.input.cbor` suffix.

```json
{
  "version": "2",
  "generated_at": 1793269518604,
  "protocol_version": "11.0",
  "dataset_commit": "f7b3b4e7f8cbf9b585f82153d60b3f6065a5b513",
  "implementation_commit": "a0e1329536b437f5c1ef255df8e60fd8c101380c",
  "results": {
    "auxiliary_data/valid/00003-2f2bacfd41c291d4.input.cbor": true,
    "auxiliary_data/invalid-zap-1/00001-example.input.cbor": "decode: decoded successfully, should be rejected"
  }
}
```
