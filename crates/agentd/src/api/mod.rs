//! Superfície HTTP do daemon, servida sobre Unix socket.

pub mod error;
pub mod routes;
pub mod types;

pub use error::ApiResult;
pub use types::{PROTOCOL_HEADER, PROTOCOL_VERSION};
