//! Durable, append-only session persistence.
//!
//! Everything the harness remembers lives in a single `session.csv` file, one
//! event per row, RFC 4180 encoded. There is no `history/` directory and no
//! `state.json`: the state is reconstructed by reading the CSV back, and file
//! history is Git's job.

pub mod csv;
pub mod event;
pub mod manager;
pub mod store;

pub use event::{decode_args, encode_args, encode_json_args, Event, EventType, Status};
pub use manager::{SessionInfo, SessionManager};
pub use store::SessionStore;
