//! Authentication feature.
//!
//! Contains the stateless JWT access-token service and its
//! `AuthenticatedPrincipal` extractor (AUTH-06), the refresh-session store whose
//! HTTP routes are delivered by AUTH-07, and the MySQL identity model consumed by
//! signup (AUTH-05) and OAuth login (AUTH-08, AUTH-09).

// The service is wired into `AppState`; issuance is called by login and refresh
// (AUTH-05/07) and the extractor by role guards (AUTH-04), which have no callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod access_token;
// The identity model is the contract for AUTH-05/08/09 and has no in-process
// callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod identity;
// The store is the contract for AUTH-07 and has no in-process callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod refresh_store;
