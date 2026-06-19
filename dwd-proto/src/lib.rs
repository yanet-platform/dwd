//! gRPC API definitions for DWD, generated from `proto/dwd.proto` via tonic.
//!
//! This crate is the single boundary the CLI/TUI uses to talk to the
//! load-generating core. It contains only the generated wire types plus the
//! tonic client/server stubs — no engine, stats, or transport logic.

pub mod pb {
    //! Generated protobuf/tonic items for the `dwd.v1` package.
    tonic::include_proto!("dwd.v1");
}

pub use pb::*;
