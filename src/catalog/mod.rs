//! Local event log.
//!
//! v1 design: the node ingests TDF blobs on its own authority and appends a
//! `ContentEvent` to a redb-backed log. Readers subscribe via
//! `tdf/catalog/1` and get a CWT-filtered live stream.

pub mod store;
pub mod types;

pub use types::{ContentEvent, NewContentEvent};
