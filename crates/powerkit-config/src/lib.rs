//! PowerKit configuration: one typed, versioned JSON document plus small per-feature
//! state files. See `docs/ARCHITECTURE.md` ("Configuration") for the design rationale.

pub mod schema;
pub mod store;

pub use schema::*;
pub use store::{write_atomic, ConfigStore, LoadReport, LoadSource, Paths, StateFile, StoreError};
