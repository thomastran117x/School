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
        "12" \
        "SELECT COUNT(*) FROM information_schema.columns WHERE table_schema = DATABASE() AND data_type = 'binary' AND character_maximum_length = 16 AND CONCAT(table_name, '.', column_name) IN ('users.id', 'local_credentials.user_id', 'external_identities.id', 'external_identities.user_id', 'courses.id', 'courses.owner_user_id', 'applications.id', 'applications.student_id', 'applications.course_id', 'application_reviews.id', 'application_reviews.application_id', 'application_reviews.reviewer_id');"

    assert_scalar \
        "password hash columns (only in local_credentials)" \
        "local_credentials" \
        "SELECT GROUP_CONCAT(table_name) FROM information_schema.columns WHERE table_schema = DATABASE() AND column_name = 'password_hash';"

    assert_scalar \
        "audit-preserving foreign keys" \
        "5" \
        "SELECT COUNT(*) FROM information_schema.referential_constraints WHERE constraint_schema = DATABASE() AND constraint_name IN ('fk_courses_owner', 'fk_applications_student', 'fk_applications_course', 'fk_application_reviews_application', 'fk_application_reviews_reviewer') AND update_rule = 'RESTRICT' AND delete_rule = 'RESTRICT';"

    assert_scalar \
        "auth-only cascading foreign keys" \
        "2" \
        "SELECT COUNT(*) FROM information_schema.referential_constraints WHERE constraint_schema = DATABASE() AND constraint_name IN ('fk_external_identities_user', 'fk_local_credentials_user') AND update_rule = 'RESTRICT' AND delete_rule = 'CASCADE';"

    assert_scalar \
        "all foreign keys" \
        "7" \
        "SELECT COUNT(*) FROM information_schema.referential_constraints WHERE constraint_schema = DATABASE();"

    assert_scalar \
        "named check constraints" \
        "20" \
        "SELECT COUNT(*) FROM information_schema.table_constraints WHERE constraint_schema = DATABASE() AND constraint_type = 'CHECK' AND constraint_name IN ('ck_users_role', 'ck_users_account_status', 'ck_users_deleted_state', 'ck_users_deletion_requested', 'ck_users_email_presence', 'ck_users_verification_state', 'ck_local_credentials_algorithm', 'ck_local_credentials_hash_format', 'ck_external_identities_provider', 'ck_external_identities_subject_not_empty', 'ck_external_identities_tenant', 'ck_courses_status', 'ck_courses_title_not_blank', 'ck_courses_code_not_blank', 'ck_courses_term_not_blank', 'ck_courses_description_length', 'ck_courses_published_state', 'ck_courses_archived_state', 'ck_applications_status', 'ck_application_reviews_decision');"

    assert_scalar \
        "domain uniqueness constraints" \
        "6" \
        "SELECT COUNT(*) FROM information_schema.table_constraints WHERE constraint_schema = DATABASE() AND constraint_type = 'UNIQUE' AND constraint_name IN ('uq_users_normalized_email', 'uq_external_identities_provider_subject', 'uq_external_identities_user_provider', 'uq_courses_owner_code_term', 'uq_applications_student_course', 'uq_application_reviews_application_id');"

    assert_scalar \
        "query indexes" \
        "7" \
        "SELECT COUNT(DISTINCT index_name) FROM information_schema.statistics WHERE table_schema = DATABASE() AND index_name IN ('idx_users_deletion_queue', 'idx_courses_owner_status', 'idx_courses_status_created_at', 'idx_applications_student_submitted_at', 'idx_applications_course_status', 'idx_applications_status_submitted_at', 'idx_application_reviews_reviewer_reviewed_at');"
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
        INSERT INTO courses (id, owner_user_id, title, code, term, description)
        VALUES (UNHEX('00000000000070008000000000000021'), UNHEX('00000000000070008000000000000002'), 'Introduction to Computing', 'CSC101', 'Fall 2030', 'Foundational computing course');
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

        -- Course uniqueness is per owner: another owner may reuse a code and term,
        -- courses without a code are unconstrained, and a code may repeat across terms.
        INSERT INTO courses (id, owner_user_id, title, code, term, description)
        VALUES (UNHEX('00000000000070008000000000000061'), UNHEX('00000000000070008000000000000001'), 'Parallel Section', 'CSC101', 'Fall 2030', '');
        INSERT INTO courses (id, owner_user_id, title, description)
        VALUES (UNHEX('00000000000070008000000000000062'), UNHEX('00000000000070008000000000000002'), 'Uncoded A', '');
        INSERT INTO courses (id, owner_user_id, title, description)
        VALUES (UNHEX('00000000000070008000000000000063'), UNHEX('00000000000070008000000000000002'), 'Uncoded B', '');
        INSERT INTO courses (id, owner_user_id, title, code, term, description)
        VALUES (UNHEX('00000000000070008000000000000064'), UNHEX('00000000000070008000000000000002'), 'Next Offering', 'CSC101', 'Winter 2031', '');
        INSERT INTO courses (id, owner_user_id, title, code, description)
        VALUES (UNHEX('00000000000070008000000000000065'), UNHEX('00000000000070008000000000000002'), 'Termless', 'MAT200', '');
        INSERT INTO courses (id, owner_user_id, title, description)
        VALUES (UNHEX('00000000000070008000000000000066'), UNHEX('00000000000070008000000000000007'), 'Owned by OAuth user', REPEAT('d', 10000));
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
        "course lifecycle defaults" \
        "DRAFT:1:1" \
        "SELECT CONCAT(status, ':', published_at IS NULL, ':', archived_at IS NULL) FROM courses WHERE id = UNHEX('00000000000070008000000000000021');"
    assert_scalar \
        "courses sharing CSC101 across owners and terms" \
        "3" \
        "SELECT COUNT(*) FROM courses WHERE code = 'csc101';"
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
        "an owner reusing a course code and term case-insensitively" \
        "INSERT INTO courses (id, owner_user_id, title, code, term, description) VALUES (UNHEX('00000000000070008000000000000070'), UNHEX('00000000000070008000000000000002'), 'Duplicate', 'csc101', 'fall 2030', '');"
    expect_rejected \
        "an owner reusing a course code without a term" \
        "INSERT INTO courses (id, owner_user_id, title, code, description) VALUES (UNHEX('00000000000070008000000000000071'), UNHEX('00000000000070008000000000000002'), 'Duplicate', 'mat200', '');"
    expect_rejected \
        "a course for an unknown owner" \
        "INSERT INTO courses (id, owner_user_id, title, description) VALUES (UNHEX('00000000000070008000000000000072'), UNHEX('00000000000070008000000000000099'), 'Orphan', '');"
    expect_rejected \
        "an empty course title" \
        "INSERT INTO courses (id, owner_user_id, title, description) VALUES (UNHEX('00000000000070008000000000000073'), UNHEX('00000000000070008000000000000002'), '', '');"
    expect_rejected \
        "a whitespace-only course title" \
        "INSERT INTO courses (id, owner_user_id, title, description) VALUES (UNHEX('00000000000070008000000000000073'), UNHEX('00000000000070008000000000000002'), '   ', '');"
    expect_rejected \
        "an over-long course title" \
        "INSERT INTO courses (id, owner_user_id, title, description) VALUES (UNHEX('00000000000070008000000000000073'), UNHEX('00000000000070008000000000000002'), REPEAT('t', 201), '');"
    expect_rejected \
        "a whitespace-only course code" \
        "INSERT INTO courses (id, owner_user_id, title, code, description) VALUES (UNHEX('00000000000070008000000000000074'), UNHEX('00000000000070008000000000000002'), 'Blank Code', '  ', '');"
    expect_rejected \
        "an empty course term" \
        "INSERT INTO courses (id, owner_user_id, title, term, description) VALUES (UNHEX('00000000000070008000000000000075'), UNHEX('00000000000070008000000000000002'), 'Blank Term', '', '');"
    expect_rejected \
        "an over-long course description" \
        "INSERT INTO courses (id, owner_user_id, title, description) VALUES (UNHEX('00000000000070008000000000000076'), UNHEX('00000000000070008000000000000002'), 'Long', REPEAT('d', 10001));"
    expect_rejected \
        "an invalid course status" \
        "INSERT INTO courses (id, owner_user_id, title, description, status) VALUES (UNHEX('00000000000070008000000000000077'), UNHEX('00000000000070008000000000000002'), 'Bad Status', '', 'DELETED');"
    expect_rejected \
        "a non-canonical course status" \
        "INSERT INTO courses (id, owner_user_id, title, description, status) VALUES (UNHEX('00000000000070008000000000000077'), UNHEX('00000000000070008000000000000002'), 'Bad Status', '', 'draft');"
    expect_rejected \
        "a published course without a publication time" \
        "UPDATE courses SET status = 'PUBLISHED' WHERE id = UNHEX('00000000000070008000000000000062');"
    expect_rejected \
        "a draft course with a publication time" \
        "UPDATE courses SET published_at = CURRENT_TIMESTAMP(6) WHERE id = UNHEX('00000000000070008000000000000062');"
    expect_rejected \
        "an archived course without an archive time" \
        "UPDATE courses SET status = 'ARCHIVED' WHERE id = UNHEX('00000000000070008000000000000062');"
    expect_rejected \
        "an archive time on a course that is not archived" \
        "UPDATE courses SET archived_at = CURRENT_TIMESTAMP(6) WHERE id = UNHEX('00000000000070008000000000000062');"
    expect_rejected \
        "a direct write to the generated term key" \
        "INSERT INTO courses (id, owner_user_id, title, description, term_key) VALUES (UNHEX('00000000000070008000000000000078'), UNHEX('00000000000070008000000000000002'), 'Term Key', '', 'x');"

    # Publishing, archiving, and restoring are status changes that keep history.
    mysql_query "
        UPDATE courses SET status = 'PUBLISHED', published_at = CURRENT_TIMESTAMP(6) WHERE id = UNHEX('00000000000070008000000000000021');
        UPDATE courses SET status = 'ARCHIVED', archived_at = CURRENT_TIMESTAMP(6) WHERE id = UNHEX('00000000000070008000000000000021');
        UPDATE courses SET status = 'ARCHIVED', archived_at = CURRENT_TIMESTAMP(6) WHERE id = UNHEX('00000000000070008000000000000064');
    "
    assert_scalar \
        "applications of an archived course" \
        "ARCHIVED:2" \
        "SELECT CONCAT(c.status, ':', COUNT(a.id)) FROM courses c LEFT JOIN applications a ON a.course_id = c.id WHERE c.id = UNHEX('00000000000070008000000000000021') GROUP BY c.id, c.status;"
    mysql_query "UPDATE courses SET status = 'PUBLISHED', archived_at = NULL WHERE id = UNHEX('00000000000070008000000000000021');"
    assert_scalar \
        "a restored course keeps its first publication time" \
        "PUBLISHED:0:1" \
        "SELECT CONCAT(status, ':', published_at IS NULL, ':', archived_at IS NULL) FROM courses WHERE id = UNHEX('00000000000070008000000000000021');"
    expect_rejected \
        "an owner reusing an archived course's code and term" \
        "INSERT INTO courses (id, owner_user_id, title, code, term, description) VALUES (UNHEX('00000000000070008000000000000079'), UNHEX('00000000000070008000000000000002'), 'Reuse', 'CSC101', 'Winter 2031', '');"
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
        "hard deletion of a user who owns a course" \
        "DELETE FROM users WHERE id = UNHEX('00000000000070008000000000000007');"
    assert_scalar \
        "identities of a course owner after a refused deletion" \
        "2" \
        "SELECT COUNT(*) FROM external_identities WHERE user_id = UNHEX('00000000000070008000000000000007');"
    expect_rejected \
        "changing a course owner to an unknown user" \
        "UPDATE courses SET owner_user_id = UNHEX('00000000000070008000000000000099') WHERE id = UNHEX('00000000000070008000000000000066');"
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
