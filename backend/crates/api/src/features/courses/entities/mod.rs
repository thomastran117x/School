//! SeaORM entities for the course tables.
//!
//! Entities do not derive `Serialize`: handlers must map them to their own
//! response models and decide explicitly which fields are public.

pub(crate) mod course;
pub(crate) mod course_membership;
