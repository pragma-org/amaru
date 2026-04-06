# Observability Macros

Procedural macros for compile-time validated tracing instrumentation.

## Macros

| Macro | Purpose |
|-------|---------|
| `define_schemas!` | Define span schemas with required/optional fields |
| `define_local_schemas!` | Same as above, but without `#[macro_export]` (for tests) |
| `trace_span!(schema, field = value, ...)` or `trace_span!(LEVEL, schema, ...)` | Create a span with schema validation and format specifiers |
| `trace_record!(schema, field1 = value1, ...)` or `trace_record!(LEVEL, schema, ...)` | Record fields to current span; optionally emit log event at LEVEL |

## Quick Example

```rust
use amaru_observability_macros::{define_schemas, trace_record, trace_span};

// 1. Define schemas
define_schemas! {
    consensus {
        chain_sync {
            VALIDATE_HEADER {
                required slot: u64
                required hash: String
                optional peer_id: String
            }
        }
    }
}

// 2. Use the schema path to create spans
use amaru::consensus::chain_sync::VALIDATE_HEADER;

fn validate_header(slot: u64, hash: String) {
    let _span = trace_span!(VALIDATE_HEADER, slot = slot, hash = hash).entered();
    // Function body
}

// Or with custom level
fn validate_header_debug(slot: u64, hash: String) {
    let _span = trace_span!(DEBUG, VALIDATE_HEADER, slot = slot, hash = hash).entered();
    // Function body
}

fn add_peer_info() {
    // Record to current span only
    trace_record!(VALIDATE_HEADER, peer_id = "some_peer");

    // Record to span AND emit info-level log event
    trace_record!(INFO, VALIDATE_HEADER, peer_id = "some_peer");

    // Create a span with custom level
    let _span = trace_span!(WARN, VALIDATE_HEADER, slot = 42).entered();
}
```

## How It Works

### What `define_schemas!` Generates

For a schema definition like:

```rust
define_schemas! {
    consensus {
        chain_sync {
            VALIDATE_HEADER {
                required { slot: u64, hash: String }
                optional { peer_id: String }
            }
        }
    }
}
```

The macro generates:

1. **Nested modules** mirroring the schema hierarchy:
   ```rust
   pub mod amaru {
       pub mod consensus {
           pub mod chain_sync {
               pub const VALIDATE_HEADER: &str = "consensus::chain_sync::VALIDATE_HEADER";
               // ... metadata and validation macros
           }
       }
   }
   ```

2. **A const** for the span name (`VALIDATE_HEADER`)

3. **Validation macros** (internal, used by `trace_span!`):
   - `__CONSENSUS__CHAIN_SYNC__VALIDATE_HEADER_INSTRUMENT!` — creates the span
   - `__CONSENSUS__CHAIN_SYNC__VALIDATE_HEADER_REQUIRE!` — validates required fields exist

4. **Registry entry** for runtime introspection (via `inventory` crate)

### What `trace_span!` Generates

The `trace_span!` macro:

1. **Validates** at compile-time that the schema const exists (typos cause `E0425`)
2. **Validates** that all required fields are provided at the call site
3. **Creates** a `tracing` span with the schema name and supplied fields

```rust
// This input:
let _span = trace_span!(VALIDATE_HEADER, slot = slot, hash = hash).entered();

// Becomes (conceptually):
let _span = tracing::info_span!("VALIDATE_HEADER", slot = %slot, hash = %hash).entered();
```

### What `trace_record!` Generates

The `trace_record!` macro records fields to the current span with a schema anchor. 
When a log level is specified, it also emits a log event at that level:

```rust
// Record to span only
trace_record!(VALIDATE_HEADER, peer_id = "peer", attempts = 3);

// Expands to:
{
    let _schema = &VALIDATE_HEADER;  // Schema anchor for documentation
    tracing::Span::current().record("peer_id", tracing::field::display(&"peer"));
    tracing::Span::current().record("attempts", tracing::field::display(&3));
}

// Record to span AND emit debug log event
trace_record!(DEBUG, VALIDATE_HEADER, peer_id = "peer", attempts = 3);

// Expands to:
{
    let _schema = &VALIDATE_HEADER;
    tracing::Span::current().record("peer_id", tracing::field::display(&"peer"));
    tracing::Span::current().record("attempts", tracing::field::display(&3));
    tracing::debug!(peer_id = %"peer", attempts = %3);  // Log event
}
```

## Compile-Time Safety

Schema typos are caught by the compiler:

```rust
trace_span!(VALIDATE_HEADER, slot = 1, hash = h)   // ✅ Compiles
trace_span!(VALIDATE_HEADERs, slot = 1, hash = h)  // ❌ error[E0425]: cannot find value `VALIDATE_HEADERs`
```

Missing required fields cause compile errors:

```rust
trace_span!(VALIDATE_HEADER, slot = 1)  // ❌ Missing required field: hash
```

Field values can be arbitrary expressions computed at the call site (e.g., `header.slot()`).

## Disabling Tracing at Compile Time

Set the `AMARU_TRACE_NO_EMIT` environment variable during compilation to disable all tracing code generation:

```bash
# Clean build required to ensure macro re-expansion
cargo clean
AMARU_TRACE_NO_EMIT=1 cargo build --release
```

When enabled:
- `define_schemas!` and `define_local_schemas!` generate no code
- `trace_span!` expands to `tracing::Span::none()`
- `trace_record!` expands to an empty block `{ }`

This completely removes tracing overhead at compile time, useful for:
- Maximum performance in production builds
- Reducing binary size
- Benchmarking without tracing interference

**Important:** `cargo clean` is required because cargo caches macro expansions. Simply setting the environment variable on an existing build won't trigger re-expansion of the macros.

## Trace Visibility

Schemas are private by default. Mark a schema `public` in `define_schemas!` if it should be documented and emitted unconditionally.

```rust
define_schemas! {
    amaru {
        security {
            /// Emitted only when AMARU_TRACE_EMIT_PRIVATE is set
            SECRET_ACCESS {
                required key_id: String
            }

            /// Always documented and emitted
            public AUDIT_EVENT {
                required actor: String
            }
        }
    }
}
```

Private schemas are not included in the runtime schema dump. They are emitted only when `AMARU_TRACE_EMIT_PRIVATE` is set to a truthy value.

```bash
AMARU_TRACE_EMIT_PRIVATE=1 AMARU_TRACE=amaru=trace ./target/release/amaru run
```

Accepted truthy values are any non-empty values except `0` and `false`.

## Architecture: Staged Macro Expansion

This crate uses a **hybrid approach** where procedural macros generate calls to declarative macros. This solves a fundamental problem: proc macros run early (before schemas are available), but we need to validate against schema data.

### Why Two Macro Types?

| Macro Type | When It Runs | What It Can Do |
|------------|--------------|----------------|
| **Procedural** (`trace_span!`) | Early, before other code | Parse call sites, validate schema paths, generate code |
| **Declarative** (`macro_rules!`) | Late, after all code | See generated validators, do pattern matching |

The proc macro **cannot** see schema data directly (it doesn't exist yet). But it **can** emit calls to declarative macros that will expand later, when the schema data is available.

### Three-Stage Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│ STAGE 1: define_schemas! (proc macro)                           │
│                                                                 │
│ Parses schema definitions and generates:                        │
│ - Nested modules with const paths                               │
│ - Declarative validator macros (macro_rules!)                   │
│   └─ __VALIDATE_HEADER_REQUIRE!(field1, field2, ...)           │
│   └─ __VALIDATE_HEADER_INSTRUMENT!(field = value, ...)         │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ STAGE 2: trace_span! (proc macro)                               │
│                                                                 │
│ Parses the call site and generates:                             │
│ - Calls to the validator macros from Stage 1                    │
│ - Tracing span creation with supplied fields                    │
│                                                                 │
│ Example output:                                                 │
│   __VALIDATE_HEADER_REQUIRE!(slot, hash);                       │
│   __VALIDATE_HEADER_INSTRUMENT!(slot = slot, hash = hash);      │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ STAGE 3: Declarative macro expansion                            │
│                                                                 │
│ The validator macros expand and check:                          │
│ - All required fields present → success                         │
│ - Missing field → compile_error!("Missing required field: X")   │
│ - Wrong type → compile_error!("Type mismatch...")               │
└─────────────────────────────────────────────────────────────────┘
```

### Generated Validator Macros

For each schema, `define_schemas!` generates internal macros following this pattern:

```
__<CATEGORY>__<SUBCATEGORY>__<SCHEMA>_<SUFFIX>
```

| Suffix | Purpose |
|--------|---------|
| `_REQUIRE` | Validates all required fields are present |
| `_INSTRUMENT` | Creates the span with field recording |

These are implementation details—users only interact with `trace_span!` and `trace_record!`.

### Why This Pattern?

| Alternative | Problem |
|-------------|---------|
| All in proc macro | Proc macros can't see data generated by other macros |
| All in declarative macros | Declarative macros can't parse function signatures |
| Runtime validation | Errors at runtime, not compile-time |

This "staged macro" pattern is a recognized Rust technique for compile-time validation against generated schemas.
