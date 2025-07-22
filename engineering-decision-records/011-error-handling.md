---
type: architecture
status: proposed
---

## Context

This decision record documents a general organization for how the Amaru node will handle errors.

## Motivation

Rust notoriously let users free to deal with errors. We need a consistent strategy for how our project reports, wraps, and propagates errors. Particularly as it grows in complexity, is composed of multiple crates, includes external crates, and possibly compiles to WebAssembly.
Defining precise errors and composing them without introducing too much overhead is not straightforward and requires following a consistent methodology.

## Decision

We will use the following approach:

* use `Result<T, E>` for all functions that can fail. Avoid `panic!` and related, except in truly unrecoverable (e.g. bugs) or unreachable situations
* use the [thiserror](https://github.com/dtolnay/thiserror) to define structured, descriptive error enums per crate. These errors will implement `std::error::Error`
* application-level binaries use [anyhow](https://github.com/dtolnay/anyhow) for ergonomic error propagation and context
* `?` operator usage in application binaries may wrap errors with `context()` to improve debuggability

## Consequences

* developers will need to define and maintain error enums for each crate
* anyhow is used in `amaru` crate to avoid boilerplate while retaining detailed context

## Example


```rust
use anyhow::Context;


/// In a module
// In lib.rs

// Define an error type with relevant information
// It might make sense to forward errors from nested calls
#[derive(thiserror::Error, Debug)]
#[error("Error")]
pub enum Error {
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("error processing query")]
    Query(#[source] StoreError),
}

type Result<T> = std::result::Result<T, Error>;

// If it is sufficient to use direct errors, do it
fn fn1() -> Result<()> {
    fn_with_io_error().map_err(Error::IoError)
}

// If extra contexts is required, use anyhow
fn fn1_bis() -> anyhow::Result<()> {
    fn_with_io_error().map_err(Error::IoError).context("Failed to load some specific file")
}

fn fn_with_io_error() -> std::result::Result<(), std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::Other, "Error"))
}

// In a sub-module

#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub enum OpenErrorKind {
    #[error(transparent)]
    IO(#[from] std::io::Error),
    #[error("no ledger stable snapshot found; at least one is expected")]
    NoStableSnapshot,
}

// Sometimes it makese sense to have Error close to some module sub-system
#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("error sending work unit through output port")]
    Send,
    #[error("error opening the store")]
    Open(#[source] OpenErrorKind),
}

/// In main module
// Combine fn calls by using anyhow::Result
// 
// It's assumed that calling fns do not need to handle specific errors
// If it's needed `downcast_ref` allows to cherry-pick errors
fn main() -> anyhow::Result<()> {
    fn1()?;
    fn1_bis()?;
    match fn1_bis() {
        Ok(_) => Ok::<(), anyhow::Error>(()),
        Err(e) => {
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
