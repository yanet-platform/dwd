//! Client half of the in-process gRPC seam (the TUI's view of the core).
//!
//! The server half lives in [`dwd_core::grpc`]; here we only build the client
//! channel, the [`RemoteStat`](client::RemoteStat) snapshot mirror, and the
//! driver tasks that pump stats in and forward control out.

pub mod client;
