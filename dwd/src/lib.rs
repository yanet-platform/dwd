//! The DWD binary's library: CLI parsing, the ratatui TUI, and the gRPC client
//! that drives the [`dwd_core`] engine exclusively through the [`dwd_proto`] API.
//!
//! Everything load-bearing lives in [`dwd_core`]; this crate is the frontend.

pub mod cfg;
pub mod cmd;
mod grpc;
pub mod logging;
pub mod runtime;
mod ui;
