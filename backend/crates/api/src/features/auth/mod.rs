//! Authentication feature.
//!
//! Currently contains the refresh-session store; HTTP routes that consume it are
//! delivered by AUTH-07.

// The store is the contract for AUTH-07 and has no in-process callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod refresh_store;
