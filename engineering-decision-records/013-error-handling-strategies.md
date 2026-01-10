---
type: architecture
status: proposed
---

## Context

This document records strategies for handling errors throughout the Amaru stack.

Rust notoriously let users free to deal with errors. We need a consistent strategy for how our project reports, wraps, and propagates errors. Particularly as it grows in complexity, is composed of multiple crates, includes external crates, and possibly compiles to WebAssembly.

Defining precise errors and composing them without introducing too much overhead is not straightforward and requires following a consistent methodology.

## Decision

We will use the following approach:

* use `Result<T, E>` for all functions that can fail;

* avoid `panic!` and related, except in truly unrecoverable (e.g. bugs) or unreachable situations;

* use [thiserror](https://github.com/dtolnay/thiserror) to define structured, descriptive error enums per crate. More specifically:
    * these errors will implement `std::error::Error`;
    * avoid using `#[from]` to embed internal errors (i.e errors defined in the same crate) unless there's a good reason to. `#[from]` should be preferred for
      embedding foreign errors;
    * the `#[source]` annotation should be completely unnecessary because that is already provided by `anyhow::Context`;

* Use `#[from] Box<dyn std::error::Error + Send + Sync>` if you need to embed arbitrary foreign errors of different kinds;

* when context needs to be added to an error (e.g. in application-level binaries), one shall use [anyhow](https://github.com/dtolnay/anyhow) for ergonomic error propagation and context;
    * If required, use `.downcast_ref` to pull specific errors from an `anyhow::Result` for fine-grained handling.

* `?` operator usage in application binaries may wrap errors with `context()` to improve debuggability;

## Consequences

* developers will need to define and maintain error enums for each crate;
* anyhow is used in `amaru` crate to avoid boilerplate while retaining detailed context;

## Example

<p align="right"><strong><code>lib.rs</code></strong></p>

```rust
use anyhow::Context;

// Define an error type with relevant information
// It might make sense to forward errors from nested calls
#[derive(thiserror::Error, Debug)]
#[error("Error")]
pub enum Error {
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error), // Foreign error, embedding using `#[from]`.

    #[error("error processing query")]
    Query(StoreError),
}

fn fn_with_io_error() -> Result<(), std::io::Error> {
    // some code
}

// If it is sufficient to use direct errors, do it
fn fn1() -> Result<(), Error> {
    fn_with_io_error()?;
    Ok(())
}

// If extra contexts is required, use anyhow
fn fn1_bis() -> anyhow::Result<()> {
    fn_with_io_error()?.context("failed to load some specific file")
}
```

<p align="right"><strong><code>sub-module.rs</code></strong></p>

```rust
#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    // For
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),

    #[error("error sending work unit through output port")]
    Send,

    #[error("error opening the store")]
    Open(OpenErrorKind),
}

#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub enum OpenErrorKind {
    #[error(transparent)]
    IO(#[from] std::io::Error),

    #[error("no ledger stable snapshot found; at least one is expected")]
    NoStableSnapshot,
}
```

<p align="right"><strong><code>main.rs</code></strong></p>

```rs
// Combine fn calls by using anyhow::Result
//
// It's assumed that calling fns do not need to handle specific errors
fn main() -> anyhow::Result<()> {
    fn1()?;
    fn1_bis()?;
    match fn1_bis() {
        Ok(_) => Ok::<(), anyhow::Error>(()),
        Err(e) => {
            // If needed, use `downcast_ref` allows to cherry-picking errors
            // for more fine-grained handling.
            match e.downcast_ref() {
                Some(Error::IoError(_)) => Ok(()),
                Some(_) => Ok(()),
                None => Ok(()),
            }
        }
    }?;
    Ok(())
}
```

## Discussion points

- https://gist.github.com/jeluard/495e07dd83600b46a29fc8644c8b9875
