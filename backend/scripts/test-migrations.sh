#!/usr/bin/env bash
# Verifies the complete migration lifecycle against the current Compose MySQL service.

set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
cd "${BACKEND_DIR}"

mysql_query() {
    docker compose exec -T --env "MYSQL_PWD=${MYSQL_PASSWORD:-app}" mysql mysql \
        --user="${MYSQL_USER:-app}" \
        --database="${MYSQL_DATABASE:-app}" \
        --batch \
        --skip-column-names \
        --execute "$1"
}

assert_scalar() {
    local description=$1
    local expected=$2
    local query=$3
    local actual

    actual="$(mysql_query "${query}")"
    if [[ "${actual}" != "${expected}" ]]; then
        echo "Expected ${description} to be '${expected}', got '${actual}'." >&2
        return 1
    fi
}

expect_rejected() {
    local description=$1
    local statement=$2

    if mysql_query "${statement}" >/dev/null 2>&1; then
        echo "Expected MySQL to reject ${description}." >&2
        return 1
    fi
}

assert_schema_contract() {
    assert_scalar \
        "the number of domain tables" \
        "5" \
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN ('users', 'external_identities', 'courses', 'applications', 'application_reviews');"

    assert_scalar \
        "binary UUID columns" \
        "10" \
        "SELECT COUNT(*) FROM information_schema.columns WHERE table_schema = DATABASE() AND data_type = 'binary' AND character_maximum_length = 16 AND CONCAT(table_name, '.', column_name) IN ('users.id', 'external_identities.id', 'external_identities.user_id', 'courses.id', 'applications.id', 'applications.student_id', 'applications.course_id', 'application_reviews.id', 'application_reviews.application_id', 'application_reviews.reviewer_id');"

    assert_scalar \
        "named foreign keys" \
        "5" \
        "SELECT COUNT(*) FROM information_schema.referential_constraints WHERE constraint_schema = DATABASE() AND constraint_name IN ('fk_external_identities_user', 'fk_applications_student', 'fk_applications_course', 'fk_application_reviews_application', 'fk_application_reviews_reviewer') AND update_rule = 'RESTRICT' AND delete_rule = 'RESTRICT';"

    assert_scalar \
        "named check constraints" \
        "7" \
        "SELECT COUNT(*) FROM information_schema.table_constraints WHERE constraint_schema = DATABASE() AND constraint_type = 'CHECK' AND constraint_name IN ('ck_users_role', 'ck_users_account_status', 'ck_users_deleted_state', 'ck_courses_capacity', 'ck_courses_is_active', 'ck_applications_status', 'ck_application_reviews_decision');"

    assert_scalar \
        "domain uniqueness constraints" \
        "5" \
        "SELECT COUNT(*) FROM information_schema.table_constraints WHERE constraint_schema = DATABASE() AND constraint_type = 'UNIQUE' AND constraint_name IN ('uq_users_normalized_email', 'uq_external_identities_provider_subject', 'uq_courses_code', 'uq_applications_student_course', 'uq_application_reviews_application_id');"

    assert_scalar \
        "query indexes" \
        "6" \
        "SELECT COUNT(DISTINCT index_name) FROM information_schema.statistics WHERE table_schema = DATABASE() AND index_name IN ('idx_external_identities_user_id', 'idx_courses_availability', 'idx_applications_student_submitted_at', 'idx_applications_course_status', 'idx_applications_status_submitted_at', 'idx_application_reviews_reviewer_reviewed_at');"
}

assert_data_contract() {
    mysql_query "
        INSERT INTO users (id, email, normalized_email, display_name, password_hash)
        VALUES (UNHEX('00000000000070008000000000000001'), 'Student@Example.com', 'student@example.com', 'Student', 'argon2id-hash');
        INSERT INTO users (id, email, normalized_email, display_name, role, account_status, email_verified_at)
        VALUES (UNHEX('00000000000070008000000000000002'), 'admin@example.com', 'admin@example.com', 'Administrator', 'ADMIN', 'ACTIVE', CURRENT_TIMESTAMP(6));
        INSERT INTO external_identities (id, user_id, provider, provider_subject)
        VALUES (UNHEX('00000000000070008000000000000011'), UNHEX('00000000000070008000000000000001'), 'google', 'CaseSensitiveSubject');
        INSERT INTO courses (id, code, name, description, capacity, application_deadline)
        VALUES (UNHEX('00000000000070008000000000000021'), 'CSC101', 'Introduction to Computing', 'Foundational computing course', 30, '2030-09-01 12:00:00.000000');
        INSERT INTO applications (id, student_id, course_id)
        VALUES (UNHEX('00000000000070008000000000000031'), UNHEX('00000000000070008000000000000001'), UNHEX('00000000000070008000000000000021'));
        UPDATE applications SET status = 'ACCEPTED' WHERE id = UNHEX('00000000000070008000000000000031');
        INSERT INTO applications (id, student_id, course_id)
        VALUES (UNHEX('00000000000070008000000000000035'), UNHEX('00000000000070008000000000000002'), UNHEX('00000000000070008000000000000021'));
        INSERT INTO application_reviews (id, application_id, reviewer_id, decision, comments)
        VALUES (UNHEX('00000000000070008000000000000041'), UNHEX('00000000000070008000000000000031'), UNHEX('00000000000070008000000000000002'), 'ACCEPTED', 'Meets the course requirements');
    "

    assert_scalar \
        "user defaults" \
        "STUDENT:UNVERIFIED" \
        "SELECT CONCAT(role, ':', account_status) FROM users WHERE id = UNHEX('00000000000070008000000000000001');"
    assert_scalar \
        "course active default" \
        "1" \
        "SELECT is_active FROM courses WHERE id = UNHEX('00000000000070008000000000000021');"
    assert_scalar \
        "one persisted review" \
        "1" \
        "SELECT COUNT(*) FROM application_reviews WHERE application_id = UNHEX('00000000000070008000000000000031');"

    expect_rejected \
        "a duplicate normalized email" \
        "INSERT INTO users (id, email, normalized_email, display_name) VALUES (UNHEX('00000000000070008000000000000003'), 'duplicate@example.com', 'student@example.com', 'Duplicate');"
    expect_rejected \
        "an invalid user role" \
        "INSERT INTO users (id, email, normalized_email, display_name, role) VALUES (UNHEX('00000000000070008000000000000004'), 'role@example.com', 'role@example.com', 'Invalid Role', 'SUPERUSER');"
    expect_rejected \
        "a non-canonical account status" \
        "INSERT INTO users (id, email, normalized_email, display_name, account_status) VALUES (UNHEX('00000000000070008000000000000006'), 'status@example.com', 'status@example.com', 'Invalid Status', 'active');"
    expect_rejected \
        "an inconsistent deleted account" \
        "INSERT INTO users (id, email, normalized_email, display_name, account_status) VALUES (UNHEX('00000000000070008000000000000005'), 'deleted@example.com', 'deleted@example.com', 'Deleted', 'DELETED');"
    expect_rejected \
        "a duplicate provider subject" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000012'), UNHEX('00000000000070008000000000000002'), 'google', 'CaseSensitiveSubject');"
    expect_rejected \
        "an external identity for an unknown user" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000013'), UNHEX('00000000000070008000000000000099'), 'google', 'unknown-user');"
    expect_rejected \
        "a case-insensitive duplicate course code" \
        "INSERT INTO courses (id, code, name, description, capacity, application_deadline) VALUES (UNHEX('00000000000070008000000000000022'), 'csc101', 'Duplicate', 'Duplicate code', 1, '2030-09-01 12:00:00.000000');"
    expect_rejected \
        "a zero-capacity course" \
        "INSERT INTO courses (id, code, name, description, capacity, application_deadline) VALUES (UNHEX('00000000000070008000000000000023'), 'ZERO101', 'Zero Capacity', 'Invalid capacity', 0, '2030-09-01 12:00:00.000000');"
    expect_rejected \
        "a non-boolean active value" \
        "INSERT INTO courses (id, code, name, description, capacity, application_deadline, is_active) VALUES (UNHEX('00000000000070008000000000000024'), 'ACTIVE101', 'Invalid Active', 'Invalid active flag', 1, '2030-09-01 12:00:00.000000', 2);"
    expect_rejected \
        "a duplicate student-course application" \
        "INSERT INTO applications (id, student_id, course_id) VALUES (UNHEX('00000000000070008000000000000032'), UNHEX('00000000000070008000000000000001'), UNHEX('00000000000070008000000000000021'));"
    expect_rejected \
        "an invalid application status" \
        "UPDATE applications SET status = 'WAITLISTED' WHERE id = UNHEX('00000000000070008000000000000035');"
    expect_rejected \
        "a non-canonical application status" \
        "UPDATE applications SET status = 'pending' WHERE id = UNHEX('00000000000070008000000000000035');"
    expect_rejected \
        "an application for an unknown student" \
        "INSERT INTO applications (id, student_id, course_id) VALUES (UNHEX('00000000000070008000000000000034'), UNHEX('00000000000070008000000000000099'), UNHEX('00000000000070008000000000000021'));"
    expect_rejected \
        "a second final review" \
        "INSERT INTO application_reviews (id, application_id, reviewer_id, decision) VALUES (UNHEX('00000000000070008000000000000042'), UNHEX('00000000000070008000000000000031'), UNHEX('00000000000070008000000000000002'), 'REJECTED');"
    expect_rejected \
        "an invalid review decision" \
        "INSERT INTO application_reviews (id, application_id, reviewer_id, decision) VALUES (UNHEX('00000000000070008000000000000043'), UNHEX('00000000000070008000000000000035'), UNHEX('00000000000070008000000000000002'), 'RECONSIDER');"
    expect_rejected \
        "a review for an unknown application" \
        "INSERT INTO application_reviews (id, application_id, reviewer_id, decision) VALUES (UNHEX('00000000000070008000000000000044'), UNHEX('00000000000070008000000000000099'), UNHEX('00000000000070008000000000000002'), 'REJECTED');"
    expect_rejected \
        "a review by an unknown reviewer" \
        "INSERT INTO application_reviews (id, application_id, reviewer_id, decision) VALUES (UNHEX('00000000000070008000000000000045'), UNHEX('00000000000070008000000000000035'), UNHEX('00000000000070008000000000000099'), 'REJECTED');"
    expect_rejected \
        "hard deletion of a referenced student" \
        "DELETE FROM users WHERE id = UNHEX('00000000000070008000000000000001');"
    expect_rejected \
        "hard deletion of a referenced course" \
        "DELETE FROM courses WHERE id = UNHEX('00000000000070008000000000000021');"
    expect_rejected \
        "hard deletion of a reviewed application" \
        "DELETE FROM applications WHERE id = UNHEX('00000000000070008000000000000031');"
}

echo "Applying migrations to a clean database."
cargo run --locked -p migration -- up
cargo run --locked -p migration -- status

echo "Confirming that reapplying migrations is a no-op."
cargo run --locked -p migration -- up

echo "Checking schema and constraint behavior."
assert_schema_contract
assert_data_contract

echo "Rolling back all domain migrations."
cargo run --locked -p migration -- reset
assert_scalar \
    "domain tables after rollback" \
    "0" \
    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN ('users', 'external_identities', 'courses', 'applications', 'application_reviews');"

echo "Reapplying migrations after rollback."
cargo run --locked -p migration -- up
assert_schema_contract
assert_data_contract

echo "Migration contract tests passed."
