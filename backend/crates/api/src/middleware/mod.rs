//! Cross-cutting HTTP middleware applied uniformly to the whole API.
//!
//! These layers are deliberately not features: they hold no product behavior and are
//! composed once in [`crate::app`].

pub mod cors;
pub mod error_pages;
pub mod request_context;
pub mod request_id;
pub mod security_headers;
