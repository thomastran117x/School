#!/usr/bin/env bash
# Verifies the complete migration lifecycle against the current Compose MySQL service.

set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
cd "${BACKEND_DIR}"

# Single-quoted so the PHC `$` separators are not expanded by Bash.
ARGON2ID_HASH='$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNo'
BCRYPT_HASH='$2b$12$abcdefghijklmnopqrstuuK1Zb5n2a6m0uS4r7nWQh3x9yJvP2Gq'

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

DOMAIN_TABLES="'users', 'local_credentials', 'external_identities', 'courses', 'applications', 'application_reviews'"

assert_schema_contract() {
    assert_scalar \
        "the number of domain tables" \
        "6" \
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN (${DOMAIN_TABLES});"

    assert_scalar \
        "binary UUID columns" \
        "11" \
        "SELECT COUNT(*) FROM information_schema.columns WHERE table_schema = DATABASE() AND data_type = 'binary' AND character_maximum_length = 16 AND CONCAT(table_name, '.', column_name) IN ('users.id', 'local_credentials.user_id', 'external_identities.id', 'external_identities.user_id', 'courses.id', 'applications.id', 'applications.student_id', 'applications.course_id', 'application_reviews.id', 'application_reviews.application_id', 'application_reviews.reviewer_id');"

    assert_scalar \
        "password hash columns (only in local_credentials)" \
        "local_credentials" \
        "SELECT GROUP_CONCAT(table_name) FROM information_schema.columns WHERE table_schema = DATABASE() AND column_name = 'password_hash';"

    assert_scalar \
        "audit-preserving foreign keys" \
        "4" \
        "SELECT COUNT(*) FROM information_schema.referential_constraints WHERE constraint_schema = DATABASE() AND constraint_name IN ('fk_applications_student', 'fk_applications_course', 'fk_application_reviews_application', 'fk_application_reviews_reviewer') AND update_rule = 'RESTRICT' AND delete_rule = 'RESTRICT';"

    assert_scalar \
        "auth-only cascading foreign keys" \
        "2" \
        "SELECT COUNT(*) FROM information_schema.referential_constraints WHERE constraint_schema = DATABASE() AND constraint_name IN ('fk_external_identities_user', 'fk_local_credentials_user') AND update_rule = 'RESTRICT' AND delete_rule = 'CASCADE';"

    assert_scalar \
        "all foreign keys" \
        "6" \
        "SELECT COUNT(*) FROM information_schema.referential_constraints WHERE constraint_schema = DATABASE();"

    assert_scalar \
        "named check constraints" \
        "15" \
        "SELECT COUNT(*) FROM information_schema.table_constraints WHERE constraint_schema = DATABASE() AND constraint_type = 'CHECK' AND constraint_name IN ('ck_users_role', 'ck_users_account_status', 'ck_users_deleted_state', 'ck_users_deletion_requested', 'ck_users_email_presence', 'ck_users_verification_state', 'ck_local_credentials_algorithm', 'ck_local_credentials_hash_format', 'ck_external_identities_provider', 'ck_external_identities_subject_not_empty', 'ck_external_identities_tenant', 'ck_courses_capacity', 'ck_courses_is_active', 'ck_applications_status', 'ck_application_reviews_decision');"

    assert_scalar \
        "domain uniqueness constraints" \
        "6" \
        "SELECT COUNT(*) FROM information_schema.table_constraints WHERE constraint_schema = DATABASE() AND constraint_type = 'UNIQUE' AND constraint_name IN ('uq_users_normalized_email', 'uq_external_identities_provider_subject', 'uq_external_identities_user_provider', 'uq_courses_code', 'uq_applications_student_course', 'uq_application_reviews_application_id');"

    assert_scalar \
        "query indexes" \
        "6" \
        "SELECT COUNT(DISTINCT index_name) FROM information_schema.statistics WHERE table_schema = DATABASE() AND index_name IN ('idx_users_deletion_queue', 'idx_courses_availability', 'idx_applications_student_submitted_at', 'idx_applications_course_status', 'idx_applications_status_submitted_at', 'idx_application_reviews_reviewer_reviewed_at');"
}

assert_data_contract() {
    mysql_query "
        INSERT INTO users (id, email, normalized_email, display_name)
        VALUES (UNHEX('00000000000070008000000000000001'), 'Student@Example.com', 'student@example.com', 'Student');
        INSERT INTO local_credentials (user_id, password_hash)
        VALUES (UNHEX('00000000000070008000000000000001'), '${ARGON2ID_HASH}');
        INSERT INTO users (id, email, normalized_email, display_name, role, account_status, email_verified_at)
        VALUES (UNHEX('00000000000070008000000000000002'), 'admin@example.com', 'admin@example.com', 'Administrator', 'ADMIN', 'ACTIVE', CURRENT_TIMESTAMP(6));
        INSERT INTO external_identities (id, user_id, provider, provider_subject)
        VALUES (UNHEX('00000000000070008000000000000011'), UNHEX('00000000000070008000000000000001'), 'GOOGLE', 'CaseSensitiveSubject');
        INSERT INTO courses (id, code, name, description, capacity, application_deadline)
        VALUES (UNHEX('00000000000070008000000000000021'), 'CSC101', 'Introduction to Computing', 'Foundational computing course', 30, '2030-09-01 12:00:00.000000');
        INSERT INTO applications (id, student_id, course_id)
        VALUES (UNHEX('00000000000070008000000000000031'), UNHEX('00000000000070008000000000000001'), UNHEX('00000000000070008000000000000021'));
        UPDATE applications SET status = 'ACCEPTED' WHERE id = UNHEX('00000000000070008000000000000031');
        INSERT INTO applications (id, student_id, course_id)
        VALUES (UNHEX('00000000000070008000000000000035'), UNHEX('00000000000070008000000000000002'), UNHEX('00000000000070008000000000000021'));
        INSERT INTO application_reviews (id, application_id, reviewer_id, decision, comments)
        VALUES (UNHEX('00000000000070008000000000000041'), UNHEX('00000000000070008000000000000031'), UNHEX('00000000000070008000000000000002'), 'ACCEPTED', 'Meets the course requirements');

        -- An OAuth-only account linked to two providers, one of them reusing a subject string.
        INSERT INTO users (id, email, normalized_email, display_name, account_status, email_verified_at)
        VALUES (UNHEX('00000000000070008000000000000007'), 'oauth@example.com', 'oauth@example.com', 'OAuth Only', 'ACTIVE', CURRENT_TIMESTAMP(6));
        INSERT INTO external_identities (id, user_id, provider, provider_subject)
        VALUES (UNHEX('00000000000070008000000000000014'), UNHEX('00000000000070008000000000000007'), 'GOOGLE', 'oauth-google-subject');
        INSERT INTO external_identities (id, user_id, provider, provider_subject, tenant_id)
        VALUES (UNHEX('00000000000070008000000000000015'), UNHEX('00000000000070008000000000000007'), 'MICROSOFT', 'CaseSensitiveSubject', '9188040d-6c67-4c5b-b112-36a304b66dad');

        -- Two anonymized tombstones may both have NULL emails.
        INSERT INTO users (id, display_name, account_status, email_verified_at, deletion_requested_at, deleted_at)
        VALUES (UNHEX('00000000000070008000000000000008'), 'Deleted user', 'DELETED', CURRENT_TIMESTAMP(6), CURRENT_TIMESTAMP(6), CURRENT_TIMESTAMP(6));
        INSERT INTO users (id, display_name, account_status, deletion_requested_at, deleted_at)
        VALUES (UNHEX('00000000000070008000000000000009'), 'Deleted user', 'DELETED', CURRENT_TIMESTAMP(6), CURRENT_TIMESTAMP(6));

        -- An account awaiting deletion keeps its email until anonymization.
        INSERT INTO users (id, email, normalized_email, display_name, account_status, email_verified_at, deletion_requested_at)
        VALUES (UNHEX('0000000000007000800000000000000A'), 'leaving@example.com', 'leaving@example.com', 'Leaving', 'PENDING_DELETION', CURRENT_TIMESTAMP(6), CURRENT_TIMESTAMP(6));

        -- An unreferenced account whose auth rows are removed by a hard purge.
        INSERT INTO users (id, email, normalized_email, display_name)
        VALUES (UNHEX('0000000000007000800000000000000B'), 'purge@example.com', 'purge@example.com', 'Purge Me');
        INSERT INTO local_credentials (user_id, password_hash)
        VALUES (UNHEX('0000000000007000800000000000000B'), '${ARGON2ID_HASH}');
        INSERT INTO external_identities (id, user_id, provider, provider_subject)
        VALUES (UNHEX('00000000000070008000000000000016'), UNHEX('0000000000007000800000000000000B'), 'GOOGLE', 'purge-subject');
    "

    assert_scalar \
        "user defaults" \
        "STUDENT:UNVERIFIED" \
        "SELECT CONCAT(role, ':', account_status) FROM users WHERE id = UNHEX('00000000000070008000000000000001');"
    assert_scalar \
        "local credential algorithm default" \
        "ARGON2ID" \
        "SELECT password_algorithm FROM local_credentials WHERE user_id = UNHEX('00000000000070008000000000000001');"
    assert_scalar \
        "credentials of the OAuth-only account" \
        "0" \
        "SELECT COUNT(*) FROM local_credentials WHERE user_id = UNHEX('00000000000070008000000000000007');"
    assert_scalar \
        "providers linked to the OAuth-only account" \
        "GOOGLE,MICROSOFT" \
        "SELECT GROUP_CONCAT(provider ORDER BY provider) FROM external_identities WHERE user_id = UNHEX('00000000000070008000000000000007');"
    assert_scalar \
        "anonymized tombstones" \
        "2" \
        "SELECT COUNT(*) FROM users WHERE account_status = 'DELETED' AND email IS NULL AND normalized_email IS NULL;"
    assert_scalar \
        "course active default" \
        "1" \
        "SELECT is_active FROM courses WHERE id = UNHEX('00000000000070008000000000000021');"
    assert_scalar \
        "one persisted review" \
        "1" \
        "SELECT COUNT(*) FROM application_reviews WHERE application_id = UNHEX('00000000000070008000000000000031');"

    # Users and lifecycle.
    expect_rejected \
        "a duplicate normalized email" \
        "INSERT INTO users (id, email, normalized_email, display_name) VALUES (UNHEX('00000000000070008000000000000003'), 'duplicate@example.com', 'student@example.com', 'Duplicate');"
    expect_rejected \
        "an invalid user role" \
        "INSERT INTO users (id, email, normalized_email, display_name, role) VALUES (UNHEX('00000000000070008000000000000004'), 'role@example.com', 'role@example.com', 'Invalid Role', 'SUPERUSER');"
    expect_rejected \
        "a non-canonical user role" \
        "INSERT INTO users (id, email, normalized_email, display_name, role) VALUES (UNHEX('00000000000070008000000000000004'), 'role@example.com', 'role@example.com', 'Invalid Role', 'admin');"
    expect_rejected \
        "a non-canonical account status" \
        "INSERT INTO users (id, email, normalized_email, display_name, account_status) VALUES (UNHEX('00000000000070008000000000000006'), 'status@example.com', 'status@example.com', 'Invalid Status', 'active');"
    expect_rejected \
        "an inconsistent deleted account" \
        "INSERT INTO users (id, email, normalized_email, display_name, account_status, deletion_requested_at) VALUES (UNHEX('00000000000070008000000000000005'), 'deleted@example.com', 'deleted@example.com', 'Deleted', 'DELETED', CURRENT_TIMESTAMP(6));"
    expect_rejected \
        "an active account without a verified email" \
        "INSERT INTO users (id, email, normalized_email, display_name, account_status) VALUES (UNHEX('00000000000070008000000000000050'), 'unverified@example.com', 'unverified@example.com', 'Unverified', 'ACTIVE');"
    expect_rejected \
        "an unverified account with a verification time" \
        "INSERT INTO users (id, email, normalized_email, display_name, email_verified_at) VALUES (UNHEX('00000000000070008000000000000051'), 'verified@example.com', 'verified@example.com', 'Verified', CURRENT_TIMESTAMP(6));"
    expect_rejected \
        "a pending deletion without a request time" \
        "INSERT INTO users (id, email, normalized_email, display_name, account_status, email_verified_at) VALUES (UNHEX('00000000000070008000000000000052'), 'pending@example.com', 'pending@example.com', 'Pending', 'PENDING_DELETION', CURRENT_TIMESTAMP(6));"
    expect_rejected \
        "a deletion request time on an active account" \
        "INSERT INTO users (id, email, normalized_email, display_name, account_status, email_verified_at, deletion_requested_at) VALUES (UNHEX('00000000000070008000000000000053'), 'requested@example.com', 'requested@example.com', 'Requested', 'ACTIVE', CURRENT_TIMESTAMP(6), CURRENT_TIMESTAMP(6));"
    expect_rejected \
        "a live account without an email" \
        "INSERT INTO users (id, display_name, account_status, email_verified_at) VALUES (UNHEX('00000000000070008000000000000054'), 'No Email', 'ACTIVE', CURRENT_TIMESTAMP(6));"
    expect_rejected \
        "an email without a normalized email" \
        "INSERT INTO users (id, email, display_name, account_status, deletion_requested_at, deleted_at) VALUES (UNHEX('00000000000070008000000000000055'), 'half@example.com', 'Half', 'DELETED', CURRENT_TIMESTAMP(6), CURRENT_TIMESTAMP(6));"

    # Local credentials.
    expect_rejected \
        "a second credential for one user" \
        "INSERT INTO local_credentials (user_id, password_hash) VALUES (UNHEX('00000000000070008000000000000001'), '${ARGON2ID_HASH}');"
    expect_rejected \
        "a credential for an unknown user" \
        "INSERT INTO local_credentials (user_id, password_hash) VALUES (UNHEX('00000000000070008000000000000099'), '${ARGON2ID_HASH}');"
    expect_rejected \
        "a plaintext password" \
        "INSERT INTO local_credentials (user_id, password_hash) VALUES (UNHEX('00000000000070008000000000000002'), 'correct horse battery staple');"
    expect_rejected \
        "a non-Argon2id hash" \
        "INSERT INTO local_credentials (user_id, password_hash) VALUES (UNHEX('00000000000070008000000000000002'), '${BCRYPT_HASH}');"
    expect_rejected \
        "an unknown password algorithm" \
        "INSERT INTO local_credentials (user_id, password_hash, password_algorithm) VALUES (UNHEX('00000000000070008000000000000002'), '${BCRYPT_HASH}', 'BCRYPT');"
    expect_rejected \
        "a NULL password hash" \
        "INSERT INTO local_credentials (user_id, password_hash) VALUES (UNHEX('00000000000070008000000000000002'), NULL);"

    # External identities.
    expect_rejected \
        "a duplicate provider subject" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000012'), UNHEX('00000000000070008000000000000002'), 'GOOGLE', 'CaseSensitiveSubject');"
    expect_rejected \
        "a second identity from the same provider for one user" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000017'), UNHEX('00000000000070008000000000000001'), 'GOOGLE', 'another-google-subject');"
    expect_rejected \
        "an external identity for an unknown user" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000013'), UNHEX('00000000000070008000000000000099'), 'GOOGLE', 'unknown-user');"
    expect_rejected \
        "a non-canonical provider" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000018'), UNHEX('00000000000070008000000000000002'), 'google', 'lowercase-provider');"
    expect_rejected \
        "an unsupported provider" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000018'), UNHEX('00000000000070008000000000000002'), 'GITHUB', 'github-subject');"
    expect_rejected \
        "an empty provider subject" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000018'), UNHEX('00000000000070008000000000000002'), 'GOOGLE', '');"
    expect_rejected \
        "a NULL provider subject" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject) VALUES (UNHEX('00000000000070008000000000000018'), UNHEX('00000000000070008000000000000002'), 'GOOGLE', NULL);"
    expect_rejected \
        "a tenant on a Google identity" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject, tenant_id) VALUES (UNHEX('00000000000070008000000000000018'), UNHEX('00000000000070008000000000000002'), 'GOOGLE', 'tenant-subject', 'some-tenant');"
    expect_rejected \
        "an empty Microsoft tenant" \
        "INSERT INTO external_identities (id, user_id, provider, provider_subject, tenant_id) VALUES (UNHEX('00000000000070008000000000000018'), UNHEX('00000000000070008000000000000002'), 'MICROSOFT', 'tenant-subject', '');"

    # Courses, applications, and reviews.
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

    # Deletion and foreign-key integrity.
    expect_rejected \
        "hard deletion of a referenced student" \
        "DELETE FROM users WHERE id = UNHEX('00000000000070008000000000000001');"
    assert_scalar \
        "auth rows of a referenced student after a refused deletion" \
        "1:1" \
        "SELECT CONCAT((SELECT COUNT(*) FROM local_credentials WHERE user_id = UNHEX('00000000000070008000000000000001')), ':', (SELECT COUNT(*) FROM external_identities WHERE user_id = UNHEX('00000000000070008000000000000001')));"
    expect_rejected \
        "changing a referenced user id" \
        "UPDATE users SET id = UNHEX('00000000000070008000000000000098') WHERE id = UNHEX('00000000000070008000000000000007');"
    expect_rejected \
        "hard deletion of a referenced course" \
        "DELETE FROM courses WHERE id = UNHEX('00000000000070008000000000000021');"
    expect_rejected \
        "hard deletion of a reviewed application" \
        "DELETE FROM applications WHERE id = UNHEX('00000000000070008000000000000031');"

    mysql_query "DELETE FROM users WHERE id = UNHEX('0000000000007000800000000000000B');"
    assert_scalar \
        "auth rows left after purging an unreferenced user" \
        "0:0" \
        "SELECT CONCAT((SELECT COUNT(*) FROM local_credentials WHERE user_id = UNHEX('0000000000007000800000000000000B')), ':', (SELECT COUNT(*) FROM external_identities WHERE user_id = UNHEX('0000000000007000800000000000000B')));"
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
    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN (${DOMAIN_TABLES});"

echo "Reapplying migrations after rollback."
cargo run --locked -p migration -- up
assert_schema_contract
assert_data_contract

echo "Migration contract tests passed."
