# Shape normalization vectors

One `.in.cbor` / `.out.cbor` pair per case. An independent normalizer is correct on this set when `normalize(<n>.in.cbor)`
is byte-identical to `<n>.out.cbor` for every pair. The specification these pin down is in the repository README.

The hex below is the reviewable form of the same bytes, and it is the authority: the committed files were checked
against this table by hand rather than taken on trust from the reference implementation.

| #   | Name                | In                   | Out                  | Why it is here                                  |
| --- | ------------------- | -------------------- | -------------------- | ----------------------------------------------- |
| 1   | indefinite-array    | `9f0102ff`           | `820102`             | The basic array rewrite                         |
| 2   | indefinite-map      | `bf0102ff`           | `a10102`             | The basic map rewrite                           |
| 3   | chunked-bytes       | `5f420102420304ff`   | `4401020304`         | Chunks concatenate into one definite string     |
| 4   | chunked-text        | `7f626162626364ff`   | `6461626364`         | The same for text                               |
| 5   | nested-indefinite   | `9f9f01ffbf0102ffff` | `828101a10102`       | The rewrite is recursive                        |
| 6   | empty-indef-array   | `9fff`               | `80`                 | The empty case still rewrites                   |
| 7   | empty-indef-map     | `bfff`               | `a0`                 | The empty case still rewrites                   |
| 8   | non-minimal-int     | `1805`               | `05`                 | Heads are minimised                             |
| 9   | unsorted-map-keys   | `a202010102`         | `a202010102`         | Key order is observable in Cardano, so it stays |
| 10  | duplicate-map-keys  | `a201020103`         | `a201020103`         | Duplicate keys are content, not form            |
| 11  | uint-2^64-1         | `1bffffffffffffffff` | `1bffffffffffffffff` | The top of `uint64` must not become a bignum    |
| 12  | tag-30-rational     | `d81e820102`         | `d81e820102`         | A known tag survives                            |
| 13  | tag-259-map         | `d90103a10102`       | `d90103a10102`       | A tag wrapping a definite map survives          |
| 14  | unknown-tag-1295    | `d9050f01`           | `d9050f01`           | An unrecognised tag number survives             |
| 15  | tag-over-indefinite | `d81e9f0102ff`       | `d81e820102`         | The rewrite reaches inside a tag                |
| 16  | float16             | `f93c00`             | `f93c00`             | Float widths are never shortened                |
| 17  | bignum-fits         | `c24296be`           | `1996be`             | The basic fold to a native head                 |
| 18  | negative-bignum-fits | `c3435e0f31`        | `3a005e0f31`         | Tag 3 carries the same argument as nint         |
| 19  | bignum-leading-zeros | `c2420000`          | `00`                 | The magnitude is canonicalized before the fit test |
| 20  | bignum-too-large    | `c249010000000000000000` | `c249010000000000000000` | 2^64 has no native head                |

Rows 9 to 14, row 16 and row 20 are the must-not-change cases: they are what keeps a content bug failing.
Rows 10, 11 and 14 are the ones a naive implementation is most likely to get wrong, so they carry the most weight in the set.

Rows 11 and 20 sit either side of the same boundary and are best read together. `2^64 - 1` is the largest value a
native head can carry, so it must not become a bignum; `2^64` is the smallest that cannot, so its bignum must stay.
An implementation that folds by magnitude rather than by whether the value fits will fail one or the other.
