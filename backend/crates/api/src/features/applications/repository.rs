//! Parameterized MySQL repository for applications and final reviews.

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, QueryResult, SqlErr, Statement,
};
use uuid::Uuid;

use super::models::{
    ApplicationDecision, ApplicationRepositoryError, DecideApplication, SubmitApplication,
};
use crate::infrastructure::{DatabaseClient, DatabaseError};

const APPLICATION_DUPLICATE_CONSTRAINT: &str = "uq_applications_student_course";
const REVIEW_DUPLICATE_CONSTRAINT: &str = "uq_application_reviews_application_id";

/// Persistence boundary used by application services.
#[async_trait]
pub(crate) trait ApplicationRepository: Send + Sync {
    /// Creates one pending application and returns its UUIDv7 identifier.
    async fn submit(&self, input: SubmitApplication) -> Result<Uuid, ApplicationRepositoryError>;

    /// Atomically records one final decision and returns its UUIDv7 review identifier.
    async fn decide(&self, input: DecideApplication) -> Result<Uuid, ApplicationRepositoryError>;
}

/// SeaORM implementation of [`ApplicationRepository`] for MySQL.
#[derive(Clone)]
pub(crate) struct SeaOrmApplicationRepository {
    database: DatabaseClient,
}

impl SeaOrmApplicationRepository {
    /// Creates a repository backed by the shared bounded database client.
    pub(crate) fn new(database: DatabaseClient) -> Self {
        Self { database }
    }
}

#[async_trait]
impl ApplicationRepository for SeaOrmApplicationRepository {
    async fn submit(&self, input: SubmitApplication) -> Result<Uuid, ApplicationRepositoryError> {
        let application_id = Uuid::now_v7();

        self.database
            .run_transaction(|transaction| {
                Box::pin(async move {
                    ensure_course_accepts_applications(transaction, input.course_id).await?;
                    insert_application(transaction, application_id, input).await?;
                    Ok(application_id)
                })
            })
            .await
    }

    async fn decide(&self, input: DecideApplication) -> Result<Uuid, ApplicationRepositoryError> {
        let review_id = Uuid::now_v7();

        self.database
            .run_transaction(|transaction| {
                Box::pin(async move {
                    let course_id =
                        lock_pending_application(transaction, input.application_id).await?;
                    if input.decision == ApplicationDecision::Accepted {
                        ensure_course_has_capacity(transaction, course_id).await?;
                    }
                    update_application_status(transaction, &input).await?;
                    insert_review(transaction, review_id, &input).await?;
                    Ok(review_id)
                })
            })
            .await
    }
}

async fn ensure_course_accepts_applications(
    transaction: &DatabaseTransaction,
    course_id: Uuid,
) -> Result<(), ApplicationRepositoryError> {
    let row = transaction
        .query_one(statement(
            "SELECT `is_active`, (`application_deadline` > UTC_TIMESTAMP(6)) AS `before_deadline` FROM `courses` WHERE `id` = ? FOR SHARE",
            [uuid_value(course_id)],
        ))
        .await
        .map_err(persistence_error)?
        .ok_or(ApplicationRepositoryError::CourseNotFound)?;
    let is_active: u8 = row_value(&row, "is_active")?;
    let before_deadline: i64 = row_value(&row, "before_deadline")?;

    if is_active == 1 && before_deadline == 1 {
        Ok(())
    } else {
        Err(ApplicationRepositoryError::CourseUnavailable)
    }
}

async fn insert_application(
    transaction: &DatabaseTransaction,
    application_id: Uuid,
    input: SubmitApplication,
) -> Result<(), ApplicationRepositoryError> {
    transaction
        .execute(statement(
            "INSERT INTO `applications` (`id`, `student_id`, `course_id`) VALUES (?, ?, ?)",
            [
                uuid_value(application_id),
                uuid_value(input.student_id),
                uuid_value(input.course_id),
            ],
        ))
        .await
        .map_err(|error| named_unique_or_persistence(error, APPLICATION_DUPLICATE_CONSTRAINT))?;

    Ok(())
}

async fn lock_pending_application(
    transaction: &DatabaseTransaction,
    application_id: Uuid,
) -> Result<Uuid, ApplicationRepositoryError> {
    let row = transaction
        .query_one(statement(
            "SELECT `course_id`, `status` FROM `applications` WHERE `id` = ? FOR UPDATE",
            [uuid_value(application_id)],
        ))
        .await
        .map_err(persistence_error)?
        .ok_or(ApplicationRepositoryError::ApplicationNotFound)?;
    let status: String = row_value(&row, "status")?;
    if status != "PENDING" {
        return Err(ApplicationRepositoryError::InvalidTransition);
    }

    let course_id: Vec<u8> = row_value(&row, "course_id")?;
    Uuid::from_slice(&course_id).map_err(|_| {
        persistence_error(DbErr::Type(
            "application course identifier is not a UUID".to_owned(),
        ))
    })
}

async fn ensure_course_has_capacity(
    transaction: &DatabaseTransaction,
    course_id: Uuid,
) -> Result<(), ApplicationRepositoryError> {
    let course = transaction
        .query_one(statement(
            "SELECT `capacity` FROM `courses` WHERE `id` = ? FOR UPDATE",
            [uuid_value(course_id)],
        ))
        .await
        .map_err(persistence_error)?
        .ok_or(ApplicationRepositoryError::CourseNotFound)?;
    let capacity: u32 = row_value(&course, "capacity")?;

    let accepted = transaction
        .query_all(statement(
            "SELECT `id` FROM `applications` FORCE INDEX (`idx_applications_course_status`) WHERE `course_id` = ? AND `status` = 'ACCEPTED' FOR UPDATE",
            [uuid_value(course_id)],
        ))
        .await
        .map_err(persistence_error)?;

    if accepted.len() < capacity as usize {
        Ok(())
    } else {
        Err(ApplicationRepositoryError::CapacityReached)
    }
}

async fn update_application_status(
    transaction: &DatabaseTransaction,
    input: &DecideApplication,
) -> Result<(), ApplicationRepositoryError> {
    let result = transaction
        .execute(statement(
            "UPDATE `applications` SET `status` = ?, `updated_at` = UTC_TIMESTAMP(6) WHERE `id` = ? AND `status` = 'PENDING'",
            [
                input.decision.as_database_value().into(),
                uuid_value(input.application_id),
            ],
        ))
        .await
        .map_err(persistence_error)?;

    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(ApplicationRepositoryError::InvalidTransition)
    }
}

async fn insert_review(
    transaction: &DatabaseTransaction,
    review_id: Uuid,
    input: &DecideApplication,
) -> Result<(), ApplicationRepositoryError> {
    transaction
        .execute(statement(
            "INSERT INTO `application_reviews` (`id`, `application_id`, `reviewer_id`, `decision`, `comments`) VALUES (?, ?, ?, ?, ?)",
            [
                uuid_value(review_id),
                uuid_value(input.application_id),
                uuid_value(input.reviewer_id),
                input.decision.as_database_value().into(),
                input.comments.clone().into(),
            ],
        ))
        .await
        .map_err(|error| named_unique_or_persistence(error, REVIEW_DUPLICATE_CONSTRAINT))?;

    Ok(())
}

fn statement<const N: usize>(sql: &str, values: [sea_orm::Value; N]) -> Statement {
    Statement::from_sql_and_values(DbBackend::MySql, sql, values)
}

fn uuid_value(id: Uuid) -> sea_orm::Value {
    id.as_bytes().to_vec().into()
}

fn row_value<T>(row: &QueryResult, column: &str) -> Result<T, ApplicationRepositoryError>
where
    T: sea_orm::TryGetable,
{
    row.try_get("", column).map_err(persistence_error)
}

fn named_unique_or_persistence(error: DbErr, constraint: &str) -> ApplicationRepositoryError {
    if let Some(SqlErr::UniqueConstraintViolation(message)) = error.sql_err()
        && message.contains(constraint)
    {
        return if constraint == APPLICATION_DUPLICATE_CONSTRAINT {
            ApplicationRepositoryError::DuplicateApplication
        } else {
            ApplicationRepositoryError::InvalidTransition
        };
    }

    persistence_error(error)
}

fn persistence_error(error: DbErr) -> ApplicationRepositoryError {
    DatabaseError::from_db_error(error).into()
}

#[cfg(test)]
mod tests {
    use std::{env, time::Duration};

    use sea_orm::{DbErr, QueryResult, Value};

    use super::*;
    use crate::config::{DatabaseSettings, DatabaseTlsMode};

    #[test]
    fn repository_errors_do_not_display_database_details() {
        let error = ApplicationRepositoryError::from(DatabaseError::OperationFailed(
            DbErr::Custom("mysql://user:secret@example.invalid/app SELECT *".to_owned()),
        ));

        assert_eq!(
            error.to_string(),
            "application persistence operation failed"
        );
        assert!(!error.to_string().contains("secret"));
        assert!(!error.to_string().contains("SELECT"));
    }

    #[tokio::test]
    #[ignore = "requires the integration-test MySQL service and migrated schema"]
    async fn mysql_repository_enforces_transactions_and_concurrency() {
        let database = DatabaseClient::initialize(&integration_settings())
            .await
            .expect("integration pool should initialize");
        let repository = SeaOrmApplicationRepository::new(database.clone());

        let reviewer_id = Uuid::now_v7();
        let students: Vec<Uuid> = (0..6).map(|_| Uuid::now_v7()).collect();
        insert_user(&database, reviewer_id, "ADMIN").await;
        for student_id in &students {
            insert_user(&database, *student_id, "STUDENT").await;
        }

        let missing_course = repository
            .submit(SubmitApplication {
                student_id: students[0],
                course_id: Uuid::now_v7(),
            })
            .await
            .expect_err("an unknown course should be rejected");
        assert!(matches!(
            missing_course,
            ApplicationRepositoryError::CourseNotFound
        ));

        let inactive_course = insert_course(&database, 10, false, false).await;
        let inactive = repository
            .submit(SubmitApplication {
                student_id: students[0],
                course_id: inactive_course,
            })
            .await
            .expect_err("an inactive course should be rejected");
        assert!(matches!(
            inactive,
            ApplicationRepositoryError::CourseUnavailable
        ));

        let expired_course = insert_course(&database, 10, true, true).await;
        let expired = repository
            .submit(SubmitApplication {
                student_id: students[0],
                course_id: expired_course,
            })
            .await
            .expect_err("an expired course should be rejected");
        assert!(matches!(
            expired,
            ApplicationRepositoryError::CourseUnavailable
        ));

        let general_course = insert_course(&database, 10, true, false).await;
        let first_application = repository
            .submit(SubmitApplication {
                student_id: students[0],
                course_id: general_course,
            })
            .await
            .expect("a valid application should be persisted");
        let duplicate = repository
            .submit(SubmitApplication {
                student_id: students[0],
                course_id: general_course,
            })
            .await
            .expect_err("the unique student-course constraint should reject a duplicate");
        assert!(matches!(
            duplicate,
            ApplicationRepositoryError::DuplicateApplication
        ));

        let concurrent_input = SubmitApplication {
            student_id: students[1],
            course_id: general_course,
        };
        let (left, right) = tokio::join!(
            repository.submit(concurrent_input),
            repository.submit(concurrent_input)
        );
        assert_one_success_and_error(
            left,
            right,
            |error| matches!(error, ApplicationRepositoryError::DuplicateApplication),
            "concurrent duplicate submission",
        );

        let injection_comment = "approved'); DROP TABLE users; --".to_owned();
        repository
            .decide(DecideApplication {
                application_id: first_application,
                reviewer_id,
                decision: ApplicationDecision::Accepted,
                comments: Some(injection_comment.clone()),
            })
            .await
            .expect("the valid application should be accepted");
        assert_eq!(
            query_string(
                &database,
                "SELECT `comments` FROM `application_reviews` WHERE `application_id` = ?",
                vec![uuid_value(first_application)],
                "comments",
            )
            .await,
            injection_comment
        );
        assert!(
            query_count(&database, "SELECT COUNT(*) AS `count` FROM `users`", vec![]).await >= 7
        );

        let rollback_application = repository
            .submit(SubmitApplication {
                student_id: students[2],
                course_id: general_course,
            })
            .await
            .expect("rollback fixture application should be persisted");
        insert_review_fixture(&database, Uuid::now_v7(), rollback_application, reviewer_id).await;
        let failed_decision = repository
            .decide(DecideApplication {
                application_id: rollback_application,
                reviewer_id,
                decision: ApplicationDecision::Accepted,
                comments: None,
            })
            .await
            .expect_err("a second review should fail and roll back the status update");
        assert!(matches!(
            failed_decision,
            ApplicationRepositoryError::InvalidTransition
        ));
        assert_eq!(
            application_status(&database, rollback_application).await,
            "PENDING"
        );

        let contested_application = repository
            .submit(SubmitApplication {
                student_id: students[3],
                course_id: general_course,
            })
            .await
            .expect("contested application should be persisted");
        let accept = DecideApplication {
            application_id: contested_application,
            reviewer_id,
            decision: ApplicationDecision::Accepted,
            comments: Some("accepted concurrently".to_owned()),
        };
        let reject = DecideApplication {
            decision: ApplicationDecision::Rejected,
            comments: Some("rejected concurrently".to_owned()),
            ..accept.clone()
        };
        let (left, right) = tokio::join!(repository.decide(accept), repository.decide(reject));
        assert_one_success_and_error(
            left,
            right,
            |error| matches!(error, ApplicationRepositoryError::InvalidTransition),
            "concurrent decisions",
        );
        assert_eq!(
            query_count(
                &database,
                "SELECT COUNT(*) AS `count` FROM `application_reviews` WHERE `application_id` = ?",
                vec![uuid_value(contested_application)],
            )
            .await,
            1
        );

        let capacity_course = insert_course(&database, 1, true, false).await;
        let capacity_left = repository
            .submit(SubmitApplication {
                student_id: students[4],
                course_id: capacity_course,
            })
            .await
            .expect("first capacity application should be persisted");
        let capacity_right = repository
            .submit(SubmitApplication {
                student_id: students[5],
                course_id: capacity_course,
            })
            .await
            .expect("second capacity application should be persisted");
        let (left, right) = tokio::join!(
            repository.decide(DecideApplication {
                application_id: capacity_left,
                reviewer_id,
                decision: ApplicationDecision::Accepted,
                comments: None,
            }),
            repository.decide(DecideApplication {
                application_id: capacity_right,
                reviewer_id,
                decision: ApplicationDecision::Accepted,
                comments: None,
            })
        );
        assert_one_success_and_error(
            left,
            right,
            |error| matches!(error, ApplicationRepositoryError::CapacityReached),
            "last-seat acceptance",
        );
        assert_eq!(
            query_count(
                &database,
                "SELECT COUNT(*) AS `count` FROM `applications` WHERE `course_id` = ? AND `status` = 'ACCEPTED'",
                vec![uuid_value(capacity_course)],
            )
            .await,
            1
        );
        assert_eq!(
            query_count(
                &database,
                "SELECT COUNT(*) AS `count` FROM `applications` WHERE `course_id` = ? AND `status` = 'PENDING'",
                vec![uuid_value(capacity_course)],
            )
            .await,
            1
        );
    }

    fn integration_settings() -> DatabaseSettings {
        DatabaseSettings {
            url: env::var("DATABASE_URL").expect("integration DATABASE_URL should be set"),
            min_connections: 0,
            max_connections: 10,
            connect_timeout: Duration::from_secs(2),
            acquire_timeout: Duration::from_secs(2),
            query_timeout: Duration::from_secs(5),
            tls_mode: DatabaseTlsMode::Disabled,
            tls_ca_path: None,
        }
    }

    async fn insert_user(database: &DatabaseClient, id: Uuid, role: &str) {
        let email = format!("{}@repository.test", id.simple());
        execute(
            database,
            "INSERT INTO `users` (`id`, `email`, `normalized_email`, `display_name`, `role`, `account_status`, `email_verified_at`) VALUES (?, ?, ?, 'Repository Test', ?, 'ACTIVE', UTC_TIMESTAMP(6))",
            vec![
                uuid_value(id),
                email.clone().into(),
                email.into(),
                role.into(),
            ],
        )
        .await;
    }

    async fn insert_course(
        database: &DatabaseClient,
        capacity: u32,
        active: bool,
        expired: bool,
    ) -> Uuid {
        let id = Uuid::now_v7();
        let deadline = if expired {
            "UTC_TIMESTAMP(6) - INTERVAL 1 DAY"
        } else {
            "UTC_TIMESTAMP(6) + INTERVAL 1 DAY"
        };
        let sql = format!(
            "INSERT INTO `courses` (`id`, `code`, `name`, `description`, `capacity`, `application_deadline`, `is_active`) VALUES (?, ?, 'Repository Course', 'Repository integration fixture', ?, {deadline}, ?)"
        );
        execute(
            database,
            &sql,
            vec![
                uuid_value(id),
                id.simple().to_string().into(),
                capacity.into(),
                u8::from(active).into(),
            ],
        )
        .await;
        id
    }

    async fn insert_review_fixture(
        database: &DatabaseClient,
        review_id: Uuid,
        application_id: Uuid,
        reviewer_id: Uuid,
    ) {
        execute(
            database,
            "INSERT INTO `application_reviews` (`id`, `application_id`, `reviewer_id`, `decision`) VALUES (?, ?, ?, 'REJECTED')",
            vec![
                uuid_value(review_id),
                uuid_value(application_id),
                uuid_value(reviewer_id),
            ],
        )
        .await;
    }

    async fn application_status(database: &DatabaseClient, application_id: Uuid) -> String {
        query_string(
            database,
            "SELECT `status` FROM `applications` WHERE `id` = ?",
            vec![uuid_value(application_id)],
            "status",
        )
        .await
    }

    async fn execute(database: &DatabaseClient, sql: &str, values: Vec<Value>) {
        let statement = Statement::from_sql_and_values(DbBackend::MySql, sql, values);
        database
            .run(move |connection| {
                Box::pin(async move {
                    connection.execute(statement).await?;
                    Ok(())
                })
            })
            .await
            .expect("fixture statement should succeed");
    }

    async fn query_count(database: &DatabaseClient, sql: &str, values: Vec<Value>) -> i64 {
        query_one(database, sql, values)
            .await
            .try_get("", "count")
            .expect("count should decode")
    }

    async fn query_string(
        database: &DatabaseClient,
        sql: &str,
        values: Vec<Value>,
        column: &str,
    ) -> String {
        query_one(database, sql, values)
            .await
            .try_get("", column)
            .expect("string should decode")
    }

    async fn query_one(database: &DatabaseClient, sql: &str, values: Vec<Value>) -> QueryResult {
        let statement = Statement::from_sql_and_values(DbBackend::MySql, sql, values);
        database
            .run(move |connection| Box::pin(async move { connection.query_one(statement).await }))
            .await
            .expect("fixture query should succeed")
            .expect("fixture row should exist")
    }

    fn assert_one_success_and_error<F>(
        left: Result<Uuid, ApplicationRepositoryError>,
        right: Result<Uuid, ApplicationRepositoryError>,
        expected_error: F,
        scenario: &str,
    ) where
        F: Fn(&ApplicationRepositoryError) -> bool,
    {
        match (left, right) {
            (Ok(_), Err(error)) | (Err(error), Ok(_)) if expected_error(&error) => {}
            results => {
                panic!("{scenario} should have one success and one expected error: {results:?}")
            }
        }
    }
}
