# Third-Party Notices

Amaru is distributed under the Apache License, Version 2.0. The UPLC crate has
optional arbitrary-precision integer backends selected at compile time:

| Feature | Direct packages | License |
| --- | --- | --- |
| `dashu` (default) | `dashu-base`, `dashu-int` 0.4.1 | Apache-2.0 |
| `malachite` (release artifacts) | `malachite-base`, `malachite-nz` 0.14.0 | LGPL-3.0-only |

The default UPLC build uses `dashu`. Official Amaru release binaries enable
`malachite` explicitly and are linked against it. A standalone Malachite build
uses `--no-default-features --features malachite`.

## Malachite

The `malachite` backend is enabled for official release artifacts. A build that
uses it must preserve the applicable LGPL notices and provide the corresponding
source and relinking rights.

License texts and source code are available from the upstream project:

- [LGPL-3.0](https://www.gnu.org/licenses/lgpl-3.0.html)
- [`malachite-base`](https://crates.io/crates/malachite-base)
- [`malachite-nz`](https://crates.io/crates/malachite-nz)

The exact versions used for a build are recorded in `Cargo.lock`.
