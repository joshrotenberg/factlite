//! JSON-RPC 2.0 for Factlite: a dispatch layer and a line-based server.
//!
//! This crate is a thin adapter. A request names a method and carries the
//! version-1 wire contract of the `factlite` library in its params; the
//! result is that library's wire output. Storage semantics stay in the
//! library. The interface is described in `docs/rpc.md` and designed in
//! `docs/design/json-rpc.md`.
//!
//! ```
//! use factlite_rpc::{DispatchOptions, Dispatcher};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let directory = tempfile::tempdir()?;
//! # let path = serde_json::to_string(&directory.path().join("example.sqlite"))?;
//! let mut dispatcher = Dispatcher::new(DispatchOptions::default());
//! let opened = dispatcher
//!     .call("open", Some(&format!(r#"{{"path":{path},"create":true}}"#)))
//!     .map_err(|failure| failure.message())?;
//! assert!(opened.starts_with(r#"{"handle":"h1","head":"#));
//! let facts = dispatcher
//!     .call("facts", Some(r#"{"handle":"h1"}"#))
//!     .map_err(|failure| failure.message())?;
//! assert!(facts.ends_with(r#""value":{"facts":[],"version":1}}"#));
//! # Ok(())
//! # }
//! ```

mod dispatch;
mod json;
mod server;

pub use dispatch::{DispatchOptions, Dispatcher, FACTLITE_ERROR, Failure, ORDER_DEPENDENT};
pub use server::{DEFAULT_MAX_LINE_BYTES, ServerOptions, serve};
