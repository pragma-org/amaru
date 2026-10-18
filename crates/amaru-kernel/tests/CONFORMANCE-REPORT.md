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

One JSON object, one entry per sample, with no enclosing metadata. The corpus and protocol version a run decoded
against are properties of the run, not of the file.

| part  | value                                                                                              |
|-------|----------------------------------------------------------------------------------------------------|
| key   | the sample's path inside the corpus: `<rule>/<category>/<name>.input.cbor`                         |
| value | `true` when the sample met its expectation, otherwise a string saying why it did not               |

`<category>` is `valid`, `invalid-zap-<n>`, `manual-valid` or `manual-invalid`.

```json
{
  "block/valid/00001-3d1d6d4d1c7eb48a.input.cbor": true,
  "gov_action/manual-invalid/committee-removed-repeated.input.cbor": "decode: succeeded when expected to fail",
  "metadata/manual-valid/label-max-at-0.input.cbor": "encode: reserialization differs from the expected output"
}
```

## Failure strings

A failure is reported as `<phase>: <message>`, where the phase is `decode` for anything that went wrong when decoding the
sample and `encode` for anything that went wrong when encoding it back:

| string                                                     | meaning                                                              |
|------------------------------------------------------------|----------------------------------------------------------------------|
| `decode: succeeded when expected to fail`                  | the corpus expects this sample to be rejected, and it decoded        |
| `encode: reserialization differs from the expected output` | it decoded, but came back as bytes the reference does not agree with |
| `decode: <first line of the decoder's error>`              | it failed to decode when it should have succeeded                    |

The third form keeps only the first line of the error, so a decoder error carrying a hex dump stays one line in the report.
