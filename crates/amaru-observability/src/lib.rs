pub const OPENING_CHAIN_DB: &str = "opening chain db";
pub const MIGRATING_DATABASE: &str = "migrating database";

pub mod registry;
// Include the schemas module which uses define_schemas! to generate
// the amaru module with all schema constants and validation macros
mod schemas;
pub use schemas::*;

// Re-export the macros for convenient use
pub use amaru_observability_macros::{augment_trace, define_schemas, trace};

pub mod consensus {

    pub mod diffusion {
        pub const FETCH_BLOCK: &str = "diffusion.fetch_block";
        pub const FORWARD_CHAIN: &str = "diffusion.forward_chain";
    }

    pub mod chain_sync {
        pub const RECEIVE_HEADER: &str = "chain_sync.receive_header";
        pub const RECEIVE_HEADER_DECODE_FAILED: &str = "chain_sync.receive_header.decode_failed";
        pub const SELECT_CHAIN: &str = "chain_sync.select_chain";
        pub const VALIDATE_BLOCK: &str = "chain_sync.validate_block";
        pub const VALIDATE_HEADER: &str = "chain_sync.validate_header";
    }

    pub mod validate_header {
        pub const EVOLVE_NONCE: &str = "validate_header.evolve_nonce";
        pub const VALIDATE: &str = "validate_header.validate";
    }
}
