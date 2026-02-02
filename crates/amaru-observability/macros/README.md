# Observability Macros

Procedural macros for compile-time validated tracing instrumentation.

## Macros

| Macro | Purpose |
|-------|---------|
| `define_schemas!` | Define span schemas with required/optional fields |
| `define_local_schemas!` | Same as above, but without `#[macro_export]` (for tests) |
| `#[trace(path)]` | Instrument a function, validating required fields |
| `#[augment_trace(path)]` | Add optional fields to the current span |

## Quick Example

```rust
use amaru_observability_macros::{define_schemas, trace, augment_trace};

// 1. Define schemas
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

// 2. Use the schema path to instrument functions
use amaru::consensus::chain_sync::VALIDATE_HEADER;

#[trace(VALIDATE_HEADER)]
fn validate_header(slot: u64, hash: String) {
    // Function body - span created with slot and hash recorded
}

#[augment_trace(VALIDATE_HEADER)]
fn add_peer_info(peer_id: String) {
    // Adds peer_id to the current span (optional fields only)
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

3. **Validation macros** (internal, used by `#[trace]`):
   - `__CONSENSUS__CHAIN_SYNC__VALIDATE_HEADER_INSTRUMENT!` — creates the span
   - `__CONSENSUS__CHAIN_SYNC__VALIDATE_HEADER_REQUIRE!` — validates required fields exist

4. **Registry entry** for runtime introspection (via `inventory` crate)

### What `#[trace(SCHEMA)]` Generates

The `#[trace]` attribute:

1. **Validates** at compile-time that the schema const exists (typos cause `E0425`)
2. **Validates** that all required fields are present as function parameters
3. **Wraps** the function body in a `tracing` span
4. **Records** all matching parameters as span fields

```rust
// This input:
#[trace(VALIDATE_HEADER)]
fn validate_header(slot: u64, hash: String) { /* ... */ }

// Becomes (conceptually):
fn validate_header(slot: u64, hash: String) {
    let _span = tracing::info_span!("VALIDATE_HEADER", slot = %slot, hash = %hash).entered();
    /* ... */
}
```

### What `#[augment_trace(SCHEMA)]` Generates

Similar to `#[trace]`, but:
- Does **not** create a new span
- Only allows **optional** fields (required fields belong in `#[trace]`)
- Records fields to the **current** span

## Compile-Time Safety

Schema typos are caught by the compiler:

```rust
#[trace(VALIDATE_HEADER)]   // ✅ Compiles
#[trace(VALIDATE_HEADERs)]  // ❌ error[E0425]: cannot find value `VALIDATE_HEADERs`
```

Missing required fields cause compile errors:

```rust
#[trace(VALIDATE_HEADER)]
fn bad(slot: u64) { }  // ❌ Missing required field: hash
```

Field types are also validated:

```rust
#[trace(VALIDATE_HEADER)]
fn bad(slot: String, hash: String) { }  // ❌ Type mismatch: slot expected u64, got String
```

Types can be expressions that are computed at the call site (e.g., `header.slot()`).

## Architecture: Staged Macro Expansion

This crate uses a **hybrid approach** where procedural macros generate calls to declarative macros. This solves a fundamental problem: proc macros run early (before schemas are available), but we need to validate against schema data.

### Why Two Macro Types?

| Macro Type | When It Runs | What It Can Do |
|------------|--------------|----------------|
| **Procedural** (`#[trace]`) | Early, before other code | Parse function signatures, generate code |
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
│ STAGE 2: #[trace] (proc macro)                                  │
│                                                                 │
│ Parses the function signature and generates:                    │
│ - Calls to the validator macros from Stage 1                    │
│ - Tracing instrumentation wrapper                               │
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

These are implementation details—users only interact with `#[trace]` and `#[augment_trace]`.

### Why This Pattern?

| Alternative | Problem |
|-------------|---------|
| All in proc macro | Proc macros can't see data generated by other macros |
| All in declarative macros | Declarative macros can't parse function signatures |
| Runtime validation | Errors at runtime, not compile-time |

This "staged macro" pattern is a recognized Rust technique for compile-time validation against generated schemas.
