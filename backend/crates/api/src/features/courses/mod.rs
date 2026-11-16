//! Course feature.
//!
//! Contains the MySQL course model (the `courses` table owned by the
//! `migration` crate) and the Redis cache-aside helpers used by catalogue
//! reads. The course service and routes that consume them are delivered
//! separately.
//!
//! - [`entities::course`] maps the table; it does not derive `Serialize`.
//! - [`NewCourse`] validates and bounds input and always inserts a
//!   [`CourseStatus::Draft`] with a fresh UUIDv7, so callers cannot choose an
//!   ID or skip the lifecycle.
//! - [`transition_status`] performs lifecycle moves and keeps `published_at`
//!   and `archived_at` consistent with the database checks.
//!
//! [`entities::course`]: crate::features::courses::entities::course
//! [`NewCourse`]: crate::features::courses::NewCourse
//! [`CourseStatus::Draft`]: crate::features::courses::CourseStatus::Draft
//! [`transition_status`]: crate::features::courses::transition_status
//!
//! Courses are learning courses, not application-only offerings: there is no
//! deadline or seat capacity (see "Course schema" in `backend/README.md`).

// The cache is the contract for the catalogue read service and has no callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod cache;
// The course model is the contract for the course service and has no in-process
// callers yet.
#[allow(dead_code, unused_imports)]
pub(crate) mod entities;
#[allow(dead_code, unused_imports)]
mod models;

#[allow(unused_imports)]
pub(crate) use models::{
    CourseModelError, CourseStatus, MAX_CODE_CHARS, MAX_DESCRIPTION_CHARS, MAX_TERM_CHARS,
    MAX_TITLE_CHARS, NewCourse, transition_status,
};
