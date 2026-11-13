//! Domain inputs and errors for application persistence.

use thiserror::Error;
use uuid::Uuid;

use crate::infrastructure::DatabaseError;

/// Input required to submit one course application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SubmitApplication {
    /// Authenticated student creating the application.
    pub student_id: Uuid,
    /// Course to which the student is applying.
    pub course_id: Uuid,
}

/// Final decision that can be recorded for a pending application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApplicationDecision {
    /// Accept the application and consume one course seat.
    Accepted,
    /// Reject the application without consuming capacity.
    Rejected,
}

impl ApplicationDecision {
    pub(super) const fn as_database_value(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::Rejected => "REJECTED",
        }
    }
}

/// Input required to decide one pending application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DecideApplication {
    /// Pending application to decide.
    pub application_id: Uuid,
    /// Administrator recording the decision.
    pub reviewer_id: Uuid,
    /// Accepted or rejected final state.
    pub decision: ApplicationDecision,
    /// Optional audit comment stored verbatim.
    pub comments: Option<String>,
}

/// Stable, sanitized outcomes from application persistence operations.
#[derive(Debug, Error)]
pub(crate) enum ApplicationRepositoryError {
    /// The requested course does not exist.
    #[error("course does not exist")]
    CourseNotFound,
    /// The course is inactive or its application deadline has passed.
    #[error("course is not accepting applications")]
    CourseUnavailable,
    /// This student already applied to this course.
    #[error("application already exists")]
    DuplicateApplication,
    /// The requested application does not exist.
    #[error("application does not exist")]
    ApplicationNotFound,
    /// The application is no longer pending or already has a review.
    #[error("application cannot be decided from its current state")]
    InvalidTransition,
    /// Accepting the application would exceed course capacity.
    #[error("course capacity has been reached")]
    CapacityReached,
    /// MySQL could not complete the operation.
    #[error("application persistence operation failed")]
    Persistence(#[source] DatabaseError),
}

impl From<DatabaseError> for ApplicationRepositoryError {
    fn from(error: DatabaseError) -> Self {
        Self::Persistence(error)
    }
}
