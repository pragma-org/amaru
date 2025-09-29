#[cfg(not(target_arch = "wasm32"))]
pub use opentelemetry::metrics::{Counter, Gauge, Meter};

#[cfg(target_arch = "wasm32")]
mod wasm {
    pub type Meter = ();
    pub type Gauge = ();
    pub type Counter = ();
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;
