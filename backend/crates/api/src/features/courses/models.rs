//! Course lifecycle enum, validated insert model, and status transitions.
//!
//! Limits and enum string values must match the `courses` migration exactly;
//! the unit tests below pin them.

use sea_orm::{ActiveValue::Set, IntoActiveModel, entity::prelude::*};
use thiserror::Error;

use super::entities::course;

/// Maximum title length in characters, matching `VARCHAR(200)`.
pub(crate) const MAX_TITLE_CHARS: usize = 200;
/// Maximum code length in characters, matching `VARCHAR(32)`.
pub(crate) const MAX_CODE_CHARS: usize = 32;
/// Maximum term length in characters, matching `VARCHAR(64)`.
pub(crate) const MAX_TERM_CHARS: usize = 64;
/// Maximum description length in characters, matching `ck_courses_description_length`.
pub(crate) const MAX_DESCRIPTION_CHARS: usize = 10_000;

/// Lifecycle state of a course.
///
/// Allowed moves are `DRAFT → PUBLISHED`, `DRAFT → ARCHIVED`,
/// `PUBLISHED → ARCHIVED`, and `ARCHIVED → PUBLISHED` (restore). A published
/// course never returns to `DRAFT`, so it cannot disappear from its members.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
pub enum CourseStatus {
    /// Being prepared by its owner; not visible to learners.
    #[sea_orm(string_value = "DRAFT")]
    Draft,
    /// Live and visible to learners.
    #[sea_orm(string_value = "PUBLISHED")]
    Published,
    /// Read-only history; coursework and memberships are retained.
    #[sea_orm(string_value = "ARCHIVED")]
    Archived,
}

impl CourseStatus {
    /// Whether a course in this state may move to `target`. Same-state moves are refused.
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Draft, Self::Published | Self::Archived)
                | (Self::Published, Self::Archived)
                | (Self::Archived, Self::Published)
        )
    }
}

/// Rejected course input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CourseModelError {
    /// The title is blank, too long, or contains control characters.
    #[error("course title must be 1 to 200 characters without control characters")]
    InvalidTitle,
    /// The code is blank, too long, or contains control characters.
    #[error("course code must be 1 to 32 characters without control characters")]
    InvalidCode,
    /// The term is blank, too long, or contains control characters.
    #[error("course term must be 1 to 64 characters without control characters")]
    InvalidTerm,
    /// The description exceeds the stored limit.
    #[error("course description must be at most 10000 characters")]
    DescriptionTooLong,
    /// The requested lifecycle move is not allowed.
    #[error("course cannot move from {from:?} to {to:?}")]
    InvalidTransition {
        /// Current state.
        from: CourseStatus,
        /// Requested state.
        to: CourseStatus,
    },
}

/// Trims a single-line field and checks it is non-empty, bounded, and free of
/// control characters.
fn single_line(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    let count = trimmed.chars().count();
    (count > 0 && count <= max_chars && !trimmed.chars().any(char::is_control))
        .then(|| trimmed.to_owned())
}

/// Insert model for a new course.
///
/// There is no ID or status input: the ID is a fresh UUIDv7 and every course
/// starts as [`CourseStatus::Draft`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCourse {
    owner_user_id: Uuid,
    title: String,
    code: Option<String>,
    term: Option<String>,
    description: String,
}

impl NewCourse {
    /// Validates and normalizes course input.
    ///
    /// Title, code, and term are trimmed; a supplied code or term that is blank
    /// after trimming is rejected rather than treated as absent. The description
    /// is stored as given and may be empty.
    ///
    /// # Errors
    ///
    /// Returns the [`CourseModelError`] variant for the first invalid field.
    pub fn new(
        owner_user_id: Uuid,
        title: &str,
        code: Option<&str>,
        term: Option<&str>,
        description: String,
    ) -> Result<Self, CourseModelError> {
        let title = single_line(title, MAX_TITLE_CHARS).ok_or(CourseModelError::InvalidTitle)?;
        let code = code
            .map(|code| single_line(code, MAX_CODE_CHARS).ok_or(CourseModelError::InvalidCode))
            .transpose()?;
        let term = term
            .map(|term| single_line(term, MAX_TERM_CHARS).ok_or(CourseModelError::InvalidTerm))
            .transpose()?;
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            return Err(CourseModelError::DescriptionTooLong);
        }
        Ok(Self {
            owner_user_id,
            title,
            code,
            term,
            description,
        })
    }

    /// Builds the insert, generating a UUIDv7 and leaving timestamps to MySQL.
    pub fn into_active_model(self) -> course::ActiveModel {
        course::ActiveModel {
            id: Set(Uuid::now_v7()),
            owner_user_id: Set(self.owner_user_id),
            title: Set(self.title),
            code: Set(self.code),
            term: Set(self.term),
            description: Set(self.description),
            status: Set(CourseStatus::Draft),
            ..Default::default()
        }
    }
}

/// Builds the update that moves `course` to `target` at `now` (UTC).
///
/// Only `status`, `published_at`, and `archived_at` are changed; the ID and all
/// other columns stay unchanged, and `updated_at` is maintained by MySQL. The
/// timestamps are kept consistent with the migration's lifecycle checks:
/// `published_at` is set on the first publication only, and `archived_at` is
/// set when archiving and cleared on restore.
///
/// # Errors
///
/// Returns [`CourseModelError::InvalidTransition`] when
/// [`CourseStatus::can_transition_to`] refuses the move.
pub fn transition_status(
    course: &course::Model,
    target: CourseStatus,
    now: DateTime,
) -> Result<course::ActiveModel, CourseModelError> {
    if !course.status.can_transition_to(target) {
        return Err(CourseModelError::InvalidTransition {
            from: course.status,
            to: target,
        });
    }
    let mut update = course.clone().into_active_model();
    update.status = Set(target);
    match target {
        CourseStatus::Published => {
            if course.published_at.is_none() {
                update.published_at = Set(Some(now));
            }
            if course.archived_at.is_some() {
                update.archived_at = Set(None);
            }
        }
        CourseStatus::Archived => update.archived_at = Set(Some(now)),
        CourseStatus::Draft => {}
    }
    Ok(update)
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveEnum, ActiveValue, Iterable};

    use super::*;

    fn time(seconds: i64) -> DateTime {
        ChronoDateTimeUtc::from_timestamp(seconds, 0)
            .unwrap()
            .naive_utc()
    }

    fn course_in(
        status: CourseStatus,
        published_at: Option<DateTime>,
        archived_at: Option<DateTime>,
    ) -> course::Model {
        course::Model {
            id: Uuid::now_v7(),
            owner_user_id: Uuid::now_v7(),
            title: "Intro".to_owned(),
            code: None,
            term: None,
            description: String::new(),
            status,
            published_at,
            archived_at,
            created_at: time(0),
            updated_at: time(0),
        }
    }

    #[test]
    fn status_values_match_the_migration_check_constraint() {
        let values: Vec<String> = CourseStatus::iter().map(|s| s.to_value()).collect();
        assert_eq!(values, ["DRAFT", "PUBLISHED", "ARCHIVED"]);
        assert_eq!(
            CourseStatus::try_from_value(&"ARCHIVED".to_owned()),
            Ok(CourseStatus::Archived)
        );
        assert!(CourseStatus::try_from_value(&"draft".to_owned()).is_err());
    }

    #[test]
    fn transitions_follow_the_documented_lifecycle() {
        use CourseStatus::{Archived, Draft, Published};
        let allowed = [
            (Draft, Published),
            (Draft, Archived),
            (Published, Archived),
            (Archived, Published),
        ];
        for from in CourseStatus::iter() {
            for to in CourseStatus::iter() {
                assert_eq!(
                    from.can_transition_to(to),
                    allowed.contains(&(from, to)),
                    "{from:?} -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn new_course_is_a_draft_with_a_generated_id_and_database_timestamps() {
        let owner = Uuid::now_v7();
        let active = NewCourse::new(
            owner,
            "  Introduction to Computing ",
            Some(" CSC101 "),
            Some("Fall 2026"),
            "  Kept as given  ".to_owned(),
        )
        .unwrap()
        .into_active_model();

        assert_eq!(active.status, Set(CourseStatus::Draft));
        assert_eq!(active.owner_user_id, Set(owner));
        assert_eq!(active.title, Set("Introduction to Computing".to_owned()));
        assert_eq!(active.code, Set(Some("CSC101".to_owned())));
        assert_eq!(active.term, Set(Some("Fall 2026".to_owned())));
        assert_eq!(active.description, Set("  Kept as given  ".to_owned()));
        assert!(active.published_at.is_not_set());
        assert!(active.archived_at.is_not_set());
        assert!(active.created_at.is_not_set());
        assert!(active.updated_at.is_not_set());
        let ActiveValue::Set(id) = active.id else {
            panic!("id must be generated");
        };
        assert_eq!(id.get_version_num(), 7);
    }

    #[test]
    fn code_term_and_description_are_optional() {
        let active = NewCourse::new(Uuid::now_v7(), "Seminar", None, None, String::new())
            .unwrap()
            .into_active_model();
        assert_eq!(active.code, Set(None));
        assert_eq!(active.term, Set(None));
    }

    #[test]
    fn fields_are_bounded_in_characters() {
        let owner = Uuid::now_v7();
        let new = |title: &str, code: Option<&str>, term: Option<&str>, description: String| {
            NewCourse::new(owner, title, code, term, description)
        };

        // Multibyte input is counted in characters, not bytes.
        assert!(new(&"é".repeat(MAX_TITLE_CHARS), None, None, String::new()).is_ok());
        assert!(new("t", Some(&"é".repeat(MAX_CODE_CHARS)), None, String::new()).is_ok());
        assert!(new("t", None, Some(&"é".repeat(MAX_TERM_CHARS)), String::new()).is_ok());
        assert!(new("t", None, None, "é".repeat(MAX_DESCRIPTION_CHARS)).is_ok());

        let too_long_title = "a".repeat(MAX_TITLE_CHARS + 1);
        let too_long_code = "a".repeat(MAX_CODE_CHARS + 1);
        let too_long_term = "a".repeat(MAX_TERM_CHARS + 1);
        for (result, expected) in [
            (
                new("", None, None, String::new()),
                CourseModelError::InvalidTitle,
            ),
            (
                new(" \t ", None, None, String::new()),
                CourseModelError::InvalidTitle,
            ),
            (
                new("a\u{0}b", None, None, String::new()),
                CourseModelError::InvalidTitle,
            ),
            (
                new("line\nbreak", None, None, String::new()),
                CourseModelError::InvalidTitle,
            ),
            (
                new(&too_long_title, None, None, String::new()),
                CourseModelError::InvalidTitle,
            ),
            (
                new("t", Some("  "), None, String::new()),
                CourseModelError::InvalidCode,
            ),
            (
                new("t", Some(&too_long_code), None, String::new()),
                CourseModelError::InvalidCode,
            ),
            (
                new("t", None, Some(""), String::new()),
                CourseModelError::InvalidTerm,
            ),
            (
                new("t", None, Some(&too_long_term), String::new()),
                CourseModelError::InvalidTerm,
            ),
            (
                new("t", None, None, "a".repeat(MAX_DESCRIPTION_CHARS + 1)),
                CourseModelError::DescriptionTooLong,
            ),
        ] {
            assert_eq!(result, Err(expected));
        }
    }

    #[test]
    fn publishing_records_the_first_publication_only() {
        let draft = course_in(CourseStatus::Draft, None, None);
        let update = transition_status(&draft, CourseStatus::Published, time(10)).unwrap();
        assert_eq!(update.status, Set(CourseStatus::Published));
        assert_eq!(update.published_at, Set(Some(time(10))));
        assert_eq!(update.id, ActiveValue::Unchanged(draft.id));
        assert_eq!(update.title, ActiveValue::Unchanged(draft.title.clone()));

        let archived = course_in(CourseStatus::Archived, Some(time(10)), Some(time(20)));
        let restored = transition_status(&archived, CourseStatus::Published, time(30)).unwrap();
        assert_eq!(
            restored.published_at,
            ActiveValue::Unchanged(Some(time(10)))
        );
        assert_eq!(restored.archived_at, Set(None));
    }

    #[test]
    fn archiving_keeps_the_publication_time() {
        let published = course_in(CourseStatus::Published, Some(time(10)), None);
        let update = transition_status(&published, CourseStatus::Archived, time(20)).unwrap();
        assert_eq!(update.status, Set(CourseStatus::Archived));
        assert_eq!(update.archived_at, Set(Some(time(20))));
        assert_eq!(update.published_at, ActiveValue::Unchanged(Some(time(10))));

        let draft = course_in(CourseStatus::Draft, None, None);
        let update = transition_status(&draft, CourseStatus::Archived, time(20)).unwrap();
        assert_eq!(update.published_at, ActiveValue::Unchanged(None));
        assert_eq!(update.archived_at, Set(Some(time(20))));

        // A draft archived without ever being published gets a publication time on restore.
        let archived_draft = course_in(CourseStatus::Archived, None, Some(time(20)));
        let restored =
            transition_status(&archived_draft, CourseStatus::Published, time(30)).unwrap();
        assert_eq!(restored.published_at, Set(Some(time(30))));
    }

    #[test]
    fn refused_transitions_report_both_states() {
        let published = course_in(CourseStatus::Published, Some(time(10)), None);
        assert_eq!(
            transition_status(&published, CourseStatus::Draft, time(20)),
            Err(CourseModelError::InvalidTransition {
                from: CourseStatus::Published,
                to: CourseStatus::Draft,
            })
        );
        assert!(transition_status(&published, CourseStatus::Published, time(20)).is_err());
    }
}
