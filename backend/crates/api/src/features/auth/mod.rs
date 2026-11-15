//! Authentication feature.
//!
//! Currently contains the refresh-session store, whose HTTP routes are delivered
//! by AUTH-07, and the MySQL identity model consumed by signup (AUTH-05) and
//! OAuth login (AUTH-08, AUTH-09).

// The identity model is the contract for AUTH-05/08/09 and has no in-process
// callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod identity;
// The store is the contract for AUTH-07 and has no in-process callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod refresh_store;
