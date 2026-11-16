//! Transactional course and membership writes.
//!
//! These operations keep one invariant that the schema cannot express: **the
//! course owner always holds an `INSTRUCTOR` membership of the course.**
//!
//! - [`create_course`] inserts the course and the owner's membership in one
//!   transaction.
//! - [`change_member_role`] and [`remove_member`] refuse to demote or remove the
//!   owner.
//! - [`transfer_ownership`] makes the new owner an instructor before switching
//!   `courses.owner_user_id`; the previous owner stays an instructor.
//!
//! Every operation that reads the owner locks the course row (`SELECT … FOR
//! UPDATE`) first, so ownership changes and membership changes for the same
//! course are serialized. [`add_member`] needs no lock: the primary key
//! `(course_id, user_id)` decides concurrent duplicates, and exactly one insert
//! wins.
//!
//! Each function returns `Result<Result<T, MembershipError>, DbErr>` so callers
//! can run it through [`DatabaseClient::run`]: the outer error is an
//! infrastructure failure that keeps that method's deadline and sanitized error
//! contract, and the inner error is an expected domain outcome.
//!
//! [`DatabaseClient::run`]: crate::infrastructure::DatabaseClient::run

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseTransaction, DbErr, EntityTrait,
    IntoActiveModel, QuerySelect, SqlErr, TransactionTrait, prelude::Uuid,
};
use thiserror::Error;

use super::{
    NewCourse,
    entities::{course, course_membership},
    membership::{CourseRole, NewCourseMembership},
};

/// Outer `DbErr` for infrastructure failures, inner [`MembershipError`] for
/// expected domain outcomes.
pub(crate) type StoreResult<T> = Result<Result<T, MembershipError>, DbErr>;

/// Expected, client-safe outcomes of a course or membership write.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MembershipError {
    /// The course does not exist.
    #[error("course not found")]
    CourseNotFound,
    /// The referenced user does not exist.
    #[error("user not found")]
    UnknownUser,
    /// The referenced course or user does not exist.
    #[error("course or user not found")]
    UnknownCourseOrUser,
    /// The owner already has a course with this code in this term.
    #[error("the owner already has a course with this code and term")]
    DuplicateCourseCode,
    /// The user already holds a role in this course.
    #[error("user is already a member of this course")]
    AlreadyMember,
    /// The user holds no role in this course.
    #[error("user is not a member of this course")]
    NotMember,
    /// The change would leave the course owner without an instructor membership.
    #[error("the course owner must remain an instructor")]
    OwnerMustRemainInstructor,
}

/// Maps unique and foreign-key violations to domain errors; anything else stays a `DbErr`.
fn classify<T>(
    error: DbErr,
    on_unique: MembershipError,
    on_foreign_key: MembershipError,
) -> StoreResult<T> {
    match error.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(_)) => Ok(Err(on_unique)),
        Some(SqlErr::ForeignKeyConstraintViolation(_)) => Ok(Err(on_foreign_key)),
        _ => Err(error),
    }
}

/// Rolls back `txn` and reports `outcome`.
async fn abort<T>(txn: DatabaseTransaction, outcome: StoreResult<T>) -> StoreResult<T> {
    txn.rollback().await?;
    outcome
}

/// Reads and exclusively locks a course row for the rest of the transaction.
async fn lock_course(
    txn: &DatabaseTransaction,
    course_id: Uuid,
) -> Result<Option<course::Model>, DbErr> {
    course::Entity::find_by_id(course_id)
        .lock_exclusive()
        .one(txn)
        .await
}

/// Reads and exclusively locks one membership row.
async fn lock_membership(
    txn: &DatabaseTransaction,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<Option<course_membership::Model>, DbErr> {
    course_membership::Entity::find_by_id((course_id, user_id))
        .lock_exclusive()
        .one(txn)
        .await
}

/// Creates a draft course and makes its owner an `INSTRUCTOR` member, atomically.
///
/// # Errors
///
/// Domain: [`MembershipError::DuplicateCourseCode`] or
/// [`MembershipError::UnknownUser`]; nothing is written in either case.
pub async fn create_course<C>(db: &C, new_course: NewCourse) -> StoreResult<course::Model>
where
    C: TransactionTrait,
{
    let txn = db.begin().await?;
    let course = match new_course.into_active_model().insert(&txn).await {
        Ok(course) => course,
        Err(error) => {
            let outcome = classify(
                error,
                MembershipError::DuplicateCourseCode,
                MembershipError::UnknownUser,
            );
            return abort(txn, outcome).await;
        }
    };
    let owner = NewCourseMembership::new(course.id, course.owner_user_id, CourseRole::Instructor);
    // The course was just created in this transaction, so no other membership
    // can exist yet and every failure here is an infrastructure error.
    if let Err(error) = course_membership::Entity::insert(owner.into_active_model())
        .exec_without_returning(&txn)
        .await
    {
        txn.rollback().await?;
        return Err(error);
    }
    txn.commit().await?;
    Ok(Ok(course))
}

/// Adds `user_id` to `course_id` with `role`.
///
/// Concurrent calls for the same pair are safe: exactly one succeeds and the
/// others report [`MembershipError::AlreadyMember`].
///
/// # Errors
///
/// Domain: [`MembershipError::AlreadyMember`] whatever the requested role, or
/// [`MembershipError::UnknownCourseOrUser`].
pub async fn add_member<C>(
    db: &C,
    course_id: Uuid,
    user_id: Uuid,
    role: CourseRole,
) -> StoreResult<course_membership::Model>
where
    C: ConnectionTrait,
{
    match NewCourseMembership::new(course_id, user_id, role)
        .into_active_model()
        .insert(db)
        .await
    {
        Ok(membership) => Ok(Ok(membership)),
        Err(error) => classify(
            error,
            MembershipError::AlreadyMember,
            MembershipError::UnknownCourseOrUser,
        ),
    }
}

/// Returns the user's role in the course, or `None` when they are not a member.
///
/// This is the membership-authorization lookup: a primary-key point read.
///
/// # Errors
///
/// Returns the `DbErr` when the query fails.
pub async fn member_role<C>(
    db: &C,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<Option<CourseRole>, DbErr>
where
    C: ConnectionTrait,
{
    Ok(course_membership::Entity::find_by_id((course_id, user_id))
        .one(db)
        .await?
        .map(|membership| membership.role))
}

/// Changes a member's role. Setting the current role is a no-op.
///
/// # Errors
///
/// Domain: [`MembershipError::CourseNotFound`], [`MembershipError::NotMember`],
/// or [`MembershipError::OwnerMustRemainInstructor`] when demoting the owner.
pub async fn change_member_role<C>(
    db: &C,
    course_id: Uuid,
    user_id: Uuid,
    role: CourseRole,
) -> StoreResult<()>
where
    C: TransactionTrait,
{
    let txn = db.begin().await?;
    let Some(course) = lock_course(&txn, course_id).await? else {
        return abort(txn, Ok(Err(MembershipError::CourseNotFound))).await;
    };
    if course.owner_user_id == user_id && role != CourseRole::Instructor {
        return abort(txn, Ok(Err(MembershipError::OwnerMustRemainInstructor))).await;
    }
    let Some(membership) = lock_membership(&txn, course_id, user_id).await? else {
        return abort(txn, Ok(Err(MembershipError::NotMember))).await;
    };
    if membership.role != role {
        let mut update = membership.into_active_model();
        update.role = Set(role);
        update.update(&txn).await?;
    }
    txn.commit().await?;
    Ok(Ok(()))
}

/// Removes a member from a course.
///
/// # Errors
///
/// Domain: [`MembershipError::CourseNotFound`], [`MembershipError::NotMember`],
/// or [`MembershipError::OwnerMustRemainInstructor`] when removing the owner.
pub async fn remove_member<C>(db: &C, course_id: Uuid, user_id: Uuid) -> StoreResult<()>
where
    C: TransactionTrait,
{
    let txn = db.begin().await?;
    let Some(course) = lock_course(&txn, course_id).await? else {
        return abort(txn, Ok(Err(MembershipError::CourseNotFound))).await;
    };
    if course.owner_user_id == user_id {
        return abort(txn, Ok(Err(MembershipError::OwnerMustRemainInstructor))).await;
    }
    let deleted = course_membership::Entity::delete_by_id((course_id, user_id))
        .exec(&txn)
        .await?;
    if deleted.rows_affected == 0 {
        return abort(txn, Ok(Err(MembershipError::NotMember))).await;
    }
    txn.commit().await?;
    Ok(Ok(()))
}

/// Makes `new_owner_user_id` the owner of the course.
///
/// The new owner is made an `INSTRUCTOR` first: a new membership is added, or
/// an existing `STUDENT` membership is promoted. The previous owner keeps their
/// `INSTRUCTOR` membership and can then be removed separately. Transferring to
/// the current owner is a no-op.
///
/// # Errors
///
/// Domain: [`MembershipError::CourseNotFound`] or [`MembershipError::UnknownUser`].
pub async fn transfer_ownership<C>(
    db: &C,
    course_id: Uuid,
    new_owner_user_id: Uuid,
) -> StoreResult<course::Model>
where
    C: TransactionTrait,
{
    let txn = db.begin().await?;
    let Some(course) = lock_course(&txn, course_id).await? else {
        return abort(txn, Ok(Err(MembershipError::CourseNotFound))).await;
    };
    if course.owner_user_id == new_owner_user_id {
        txn.commit().await?;
        return Ok(Ok(course));
    }
    match lock_membership(&txn, course_id, new_owner_user_id).await? {
        Some(membership) if membership.role == CourseRole::Instructor => {}
        Some(membership) => {
            let mut update = membership.into_active_model();
            update.role = Set(CourseRole::Instructor);
            update.update(&txn).await?;
        }
        None => {
            let instructor =
                NewCourseMembership::new(course_id, new_owner_user_id, CourseRole::Instructor);
            if let Err(error) = course_membership::Entity::insert(instructor.into_active_model())
                .exec_without_returning(&txn)
                .await
            {
                // The course row is locked and the membership was absent, so a
                // foreign-key failure can only mean the user does not exist.
                let outcome = classify(
                    error,
                    MembershipError::AlreadyMember,
                    MembershipError::UnknownUser,
                );
                return abort(txn, outcome).await;
            }
        }
    }
    let mut update = course.into_active_model();
    update.owner_user_id = Set(new_owner_user_id);
    let course = update.update(&txn).await?;
    txn.commit().await?;
    Ok(Ok(course))
}

#[cfg(test)]
mod tests {
    mod integration {
        use std::{env, time::Duration};

        use sea_orm::{ColumnTrait, DatabaseConnection, PaginatorTrait, QueryFilter};

        use super::super::*;
        use crate::{
            config::{DatabaseSettings, DatabaseTlsMode},
            features::auth::identity::NewStudentAccount,
            infrastructure::DatabaseClient,
        };

        async fn client() -> DatabaseClient {
            DatabaseClient::initialize(&DatabaseSettings {
                url: env::var("DATABASE_URL").expect("integration DATABASE_URL should be set"),
                min_connections: 0,
                max_connections: 24,
                connect_timeout: Duration::from_secs(2),
                acquire_timeout: Duration::from_secs(5),
                query_timeout: Duration::from_secs(15),
                tls_mode: DatabaseTlsMode::Disabled,
                tls_ca_path: None,
            })
            .await
            .expect("integration database client should initialize")
        }

        /// Runs one store operation, failing the test on an infrastructure error.
        async fn run<T, F>(client: &DatabaseClient, operation: F) -> T
        where
            T: Send,
            F: for<'a> FnOnce(
                &'a DatabaseConnection,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<T, DbErr>> + Send + 'a>,
            >,
        {
            client
                .run(operation)
                .await
                .expect("database operation should succeed")
        }

        async fn new_user(client: &DatabaseClient) -> Uuid {
            let email = format!("member-{}@example.com", Uuid::now_v7().simple());
            let user = NewStudentAccount::new(email.clone(), email, "Member".to_owned())
                .into_active_model();
            run(client, |db| {
                Box::pin(async move { user.insert(db).await.map(|user| user.id) })
            })
            .await
        }

        async fn new_course(client: &DatabaseClient, owner: Uuid) -> course::Model {
            let new = NewCourse::new(owner, "Course", None, None, String::new()).unwrap();
            run(client, |db| Box::pin(create_course(db, new)))
                .await
                .expect("course should be created")
        }

        async fn role_of(
            client: &DatabaseClient,
            course_id: Uuid,
            user_id: Uuid,
        ) -> Option<CourseRole> {
            run(client, move |db| {
                Box::pin(member_role(db, course_id, user_id))
            })
            .await
        }

        async fn memberships_of(client: &DatabaseClient, course_id: Uuid) -> u64 {
            run(client, move |db| {
                Box::pin(
                    course_membership::Entity::find()
                        .filter(course_membership::Column::CourseId.eq(course_id))
                        .count(db),
                )
            })
            .await
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test MySQL service"]
        async fn creating_a_course_makes_the_owner_an_instructor() {
            let client = client().await;
            let owner = new_user(&client).await;

            let course = new_course(&client, owner).await;

            assert_eq!(course.owner_user_id, owner);
            assert_eq!(
                role_of(&client, course.id, owner).await,
                Some(CourseRole::Instructor)
            );
            assert_eq!(memberships_of(&client, course.id).await, 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test MySQL service"]
        async fn a_refused_course_leaves_no_course_or_membership() {
            let client = client().await;
            let owner = new_user(&client).await;
            let code = Uuid::now_v7().simple().to_string();
            let new = || {
                NewCourse::new(
                    owner,
                    "Coded",
                    Some(&code),
                    Some("Fall 2030"),
                    String::new(),
                )
                .unwrap()
            };

            let first = new();
            run(&client, |db| Box::pin(create_course(db, first)))
                .await
                .expect("first course should be created");
            let duplicate = new();
            assert_eq!(
                run(&client, |db| Box::pin(create_course(db, duplicate))).await,
                Err(MembershipError::DuplicateCourseCode)
            );
            let owned = run(&client, move |db| {
                Box::pin(
                    course::Entity::find()
                        .filter(course::Column::OwnerUserId.eq(owner))
                        .count(db),
                )
            })
            .await;
            let memberships = run(&client, move |db| {
                Box::pin(
                    course_membership::Entity::find()
                        .filter(course_membership::Column::UserId.eq(owner))
                        .count(db),
                )
            })
            .await;
            assert_eq!((owned, memberships), (1, 1));

            let orphan =
                NewCourse::new(Uuid::now_v7(), "Orphan", None, None, String::new()).unwrap();
            assert_eq!(
                run(&client, |db| Box::pin(create_course(db, orphan))).await,
                Err(MembershipError::UnknownUser)
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test MySQL service"]
        async fn duplicate_and_dangling_memberships_are_refused() {
            let client = client().await;
            let owner = new_user(&client).await;
            let student = new_user(&client).await;
            let course = new_course(&client, owner).await;
            let id = course.id;

            let added = run(&client, move |db| {
                Box::pin(add_member(db, id, student, CourseRole::Student))
            })
            .await
            .expect("student should be added");
            assert_eq!(
                (added.course_id, added.user_id, added.role),
                (id, student, CourseRole::Student)
            );

            for role in [CourseRole::Student, CourseRole::Instructor] {
                assert_eq!(
                    run(&client, move |db| Box::pin(add_member(
                        db, id, student, role
                    )))
                    .await,
                    Err(MembershipError::AlreadyMember)
                );
            }
            assert_eq!(
                run(&client, move |db| Box::pin(add_member(
                    db,
                    id,
                    owner,
                    CourseRole::Student
                )))
                .await,
                Err(MembershipError::AlreadyMember)
            );
            assert_eq!(
                role_of(&client, id, owner).await,
                Some(CourseRole::Instructor)
            );

            let unknown = Uuid::now_v7();
            assert_eq!(
                run(&client, move |db| Box::pin(add_member(
                    db,
                    unknown,
                    student,
                    CourseRole::Student
                )))
                .await,
                Err(MembershipError::UnknownCourseOrUser)
            );
            assert_eq!(
                run(&client, move |db| Box::pin(add_member(
                    db,
                    id,
                    unknown,
                    CourseRole::Student
                )))
                .await,
                Err(MembershipError::UnknownCourseOrUser)
            );
            assert_eq!(role_of(&client, id, unknown).await, None);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test MySQL service"]
        async fn concurrent_duplicate_memberships_have_exactly_one_winner() {
            let client = client().await;
            let owner = new_user(&client).await;
            let student = new_user(&client).await;
            let id = new_course(&client, owner).await.id;

            let attempts: Vec<_> = (0..16)
                .map(|attempt| {
                    let client = client.clone();
                    let role = if attempt % 2 == 0 {
                        CourseRole::Student
                    } else {
                        CourseRole::Instructor
                    };
                    tokio::spawn(async move {
                        run(&client, move |db| {
                            Box::pin(add_member(db, id, student, role))
                        })
                        .await
                    })
                })
                .collect();
            let mut winners = 0;
            for attempt in attempts {
                match attempt.await.expect("task should not panic") {
                    Ok(_) => winners += 1,
                    Err(error) => assert_eq!(error, MembershipError::AlreadyMember),
                }
            }

            assert_eq!(winners, 1);
            assert_eq!(memberships_of(&client, id).await, 2);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test MySQL service"]
        async fn concurrent_distinct_members_are_all_added() {
            let client = client().await;
            let owner = new_user(&client).await;
            let id = new_course(&client, owner).await.id;
            let mut students = Vec::new();
            for _ in 0..12 {
                students.push(new_user(&client).await);
            }

            let attempts: Vec<_> = students
                .iter()
                .map(|&student| {
                    let client = client.clone();
                    tokio::spawn(async move {
                        run(&client, move |db| {
                            Box::pin(add_member(db, id, student, CourseRole::Student))
                        })
                        .await
                    })
                })
                .collect();
            for attempt in attempts {
                attempt
                    .await
                    .expect("task should not panic")
                    .expect("each distinct member should be added");
            }

            assert_eq!(memberships_of(&client, id).await, 13);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test MySQL service"]
        async fn the_owner_cannot_be_demoted_or_removed() {
            let client = client().await;
            let owner = new_user(&client).await;
            let other = new_user(&client).await;
            let id = new_course(&client, owner).await.id;
            run(&client, move |db| {
                Box::pin(add_member(db, id, other, CourseRole::Instructor))
            })
            .await
            .expect("co-instructor should be added");

            assert_eq!(
                run(&client, move |db| Box::pin(change_member_role(
                    db,
                    id,
                    owner,
                    CourseRole::Student
                )))
                .await,
                Err(MembershipError::OwnerMustRemainInstructor)
            );
            assert_eq!(
                run(&client, move |db| Box::pin(remove_member(db, id, owner))).await,
                Err(MembershipError::OwnerMustRemainInstructor)
            );
            assert_eq!(
                role_of(&client, id, owner).await,
                Some(CourseRole::Instructor)
            );

            run(&client, move |db| {
                Box::pin(change_member_role(db, id, other, CourseRole::Student))
            })
            .await
            .expect("a co-instructor may be demoted");
            assert_eq!(role_of(&client, id, other).await, Some(CourseRole::Student));
            run(&client, move |db| Box::pin(remove_member(db, id, other)))
                .await
                .expect("a non-owner may be removed");
            assert_eq!(role_of(&client, id, other).await, None);

            assert_eq!(
                run(&client, move |db| Box::pin(remove_member(db, id, other))).await,
                Err(MembershipError::NotMember)
            );
            assert_eq!(
                run(&client, move |db| Box::pin(change_member_role(
                    db,
                    id,
                    other,
                    CourseRole::Student
                )))
                .await,
                Err(MembershipError::NotMember)
            );
            let unknown = Uuid::now_v7();
            assert_eq!(
                run(&client, move |db| Box::pin(remove_member(
                    db, unknown, other
                )))
                .await,
                Err(MembershipError::CourseNotFound)
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test MySQL service"]
        async fn transferring_ownership_keeps_both_owners_instructors() {
            let client = client().await;
            let previous = new_user(&client).await;
            let next = new_user(&client).await;
            let id = new_course(&client, previous).await.id;
            run(&client, move |db| {
                Box::pin(add_member(db, id, next, CourseRole::Student))
            })
            .await
            .expect("student should be added");

            let course = run(&client, move |db| {
                Box::pin(transfer_ownership(db, id, next))
            })
            .await
            .expect("ownership should transfer");

            assert_eq!(course.owner_user_id, next);
            assert_eq!(
                role_of(&client, id, next).await,
                Some(CourseRole::Instructor)
            );
            assert_eq!(
                role_of(&client, id, previous).await,
                Some(CourseRole::Instructor)
            );
            run(&client, move |db| Box::pin(remove_member(db, id, previous)))
                .await
                .expect("the previous owner may now be removed");

            let stranger = Uuid::now_v7();
            assert_eq!(
                run(&client, move |db| Box::pin(transfer_ownership(
                    db, id, stranger
                )))
                .await,
                Err(MembershipError::UnknownUser)
            );
            assert_eq!(
                run(&client, move |db| Box::pin(transfer_ownership(
                    db, stranger, next
                )))
                .await,
                Err(MembershipError::CourseNotFound)
            );
            assert_eq!(memberships_of(&client, id).await, 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires the integration-test MySQL service"]
        async fn racing_transfer_and_removal_never_strands_the_owner() {
            let client = client().await;
            for _ in 0..10 {
                let previous = new_user(&client).await;
                let next = new_user(&client).await;
                let id = new_course(&client, previous).await.id;
                run(&client, move |db| {
                    Box::pin(add_member(db, id, next, CourseRole::Student))
                })
                .await
                .expect("student should be added");

                let transfer = {
                    let client = client.clone();
                    tokio::spawn(async move {
                        run(&client, move |db| {
                            Box::pin(transfer_ownership(db, id, next))
                        })
                        .await
                    })
                };
                let removal = {
                    let client = client.clone();
                    tokio::spawn(async move {
                        run(&client, move |db| Box::pin(remove_member(db, id, next))).await
                    })
                };
                let transferred = transfer.await.expect("task should not panic");
                let removed = removal.await.expect("task should not panic");

                assert_eq!(transferred.map(|course| course.owner_user_id), Ok(next));
                assert!(
                    matches!(
                        removed,
                        Ok(()) | Err(MembershipError::OwnerMustRemainInstructor)
                    ),
                    "{removed:?}"
                );
                assert_eq!(
                    role_of(&client, id, next).await,
                    Some(CourseRole::Instructor)
                );
                assert_eq!(
                    role_of(&client, id, previous).await,
                    Some(CourseRole::Instructor)
                );
            }
        }
    }
}
