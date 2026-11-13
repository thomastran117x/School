//! Course application persistence and transactional review decisions.
#![allow(
    dead_code,
    reason = "the persistence boundary intentionally precedes its API service consumer"
)]

pub(crate) mod models;
pub(crate) mod repository;
