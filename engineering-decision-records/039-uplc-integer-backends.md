---
type: architecture
status: accepted
---

# Pluggable UPLC Integer Backends

## Context

The UPLC evaluator performs arbitrary-precision signed and unsigned integer
arithmetic on a protocol-critical hot path. The current implementation uses
Dashu directly in several runtime, costing, flat-codec, and ledger-value
modules. That makes a backend change unnecessarily invasive and prevents a
consumer from choosing a different licensing or performance trade-off.

One alternative was considered in addition to the existing backend:

- **Malachite** is a pure-Rust arbitrary-precision implementation. It avoids
  the native GMP build, but its integer crates are LGPL-3.0-only.

## Decision

Keep Dashu as the default UPLC backend and introduce a small private facade in
`amaru-uplc::integer`. Official release builds explicitly enable Malachite.
The UPLC crate exposes two backend features:

| Feature | Backend | Default |
| --- | --- | --- |
| `dashu` | Dashu `IBig`/`UBig` | Yes |
| `malachite` | Malachite `Integer`/`Natural` | No |

The facade owns the operations that are protocol-relevant or backend-specific:
sign and zero checks, byte conversion, bit access, division with remainder,
extended GCD, and the integer operators used by the runtime. UPLC call sites
continue to use `constant::Integer`; they do not import backend traits or
types. The UPLC CBOR integer codec is also implemented against the facade.

Backend features are selected when compiling `amaru-uplc`, for example:

```text
cargo build -p amaru-uplc --no-default-features --features malachite
```

The standard and permissive build is selected explicitly with:

```text
cargo build -p amaru-uplc
```

The release workflow adds `--features amaru-uplc/malachite`. Cargo keeps the
default Dashu feature active through transitive dependencies, so the facade
gives Malachite precedence when both backend selectors are unified.

## Comparison

The following focused run used the release benchmark on an Apple M1 Max. It
used three arithmetic-heavy V2 scripts, 50 measurements per script, and kept
the 30 fastest measurements. Memory is the additional arena occupancy reported
by the existing benchmark.

| Backend | `8100C5F1...` CPU | `6D4DEA52...` CPU | `32836438...` CPU | Memory across cases |
| --- | ---: | ---: | ---: | --- |
| Dashu | 100.6 ms | 687 us | 1.05 ms | identical |
| Malachite | 21.7 ms | 699 us | 1.05 ms | identical |

This is evidence that Malachite can materially improve the bigint-dominated
outlier. It is not a corpus-wide ranking: the backend choice should still be
revalidated against the full turbo corpus, protocol conformance, and the
target platform when either backend changes.

## Consequences

- Existing UPLC behavior and call-site types remain stable while backend code is
  isolated in one module.
- Official release builds use Malachite and add LGPL obligations to those
  artifacts.
- Dashu remains the default for regular builds and is available to consumers
  that need a permissively licensed integer backend.
- The facade has a small maintenance cost and must be updated when the runtime
  starts using a new integer operation. Backend-specific behavior is tested by
  compiling both configurations.
- Distribution artifacts include `THIRD_PARTY_NOTICES.md` so the default
  Malachite build carries the applicable notices.

## References

- [UPLC integer facade](../crates/amaru-uplc/src/integer.rs)
- [UPLC Cargo features](../crates/amaru-uplc/Cargo.toml)
- [Third-party notices](../THIRD_PARTY_NOTICES.md)
- [Malachite](https://docs.rs/malachite/latest/malachite/)
- [Dashu](https://docs.rs/dashu-int/latest/dashu_int/)
- [GNU LGPL](https://www.gnu.org/licenses/lgpl-3.0.html)
