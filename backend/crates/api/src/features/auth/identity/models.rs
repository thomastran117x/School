//! Identity enums, secret value types, and insert models.
//!
//! Enum string values must match the `CHECK` constraints in the `migration`
//! crate exactly; the unit tests below pin them.

use std::fmt;

use sea_orm::{ActiveValue::Set, DeriveValueType, entity::prelude::*};
use thiserror::Error;

use super::entities::{external_identity, local_credential, user};

/// Maximum stored length of a password hash, matching `VARCHAR(255)`.
const MAX_PASSWORD_HASH_LEN: usize = 255;
/// Maximum stored length of a provider subject, matching `VARCHAR(255)`.
const MAX_PROVIDER_SUBJECT_LEN: usize = 255;
/// Maximum stored length of a Microsoft tenant ID, matching `VARCHAR(64)`.
const MAX_TENANT_ID_LEN: usize = 64;
/// PHC prefix of every accepted password hash.
const ARGON2ID_PHC_PREFIX: &str = "$argon2id$";

/// Authorization role of a user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
pub enum Role {
    /// A student; the only role public signup can create.
    #[sea_orm(string_value = "STUDENT")]
    Student,
    /// An administrator; provisioned out-of-band only.
    #[sea_orm(string_value = "ADMIN")]
    Admin,
}

/// Lifecycle state of a user account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
pub enum AccountStatus {
    /// Registered but the email address is not yet verified.
    #[sea_orm(string_value = "UNVERIFIED")]
    Unverified,
    /// Verified and allowed to sign in.
    #[sea_orm(string_value = "ACTIVE")]
    Active,
    /// Blocked by an administrator.
    #[sea_orm(string_value = "DISABLED")]
    Disabled,
    /// Deletion requested; sign-in is refused while cleanup is pending.
    #[sea_orm(string_value = "PENDING_DELETION")]
    PendingDeletion,
    /// Anonymized tombstone kept for historical references.
    #[sea_orm(string_value = "DELETED")]
    Deleted,
}

/// Supported OAuth / OpenID Connect identity providers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
pub enum IdentityProvider {
    /// Google accounts.
    #[sea_orm(string_value = "GOOGLE")]
    Google,
    /// Microsoft Entra ID and personal Microsoft accounts.
    #[sea_orm(string_value = "MICROSOFT")]
    Microsoft,
}

/// Password hashing algorithm recorded next to each hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
pub enum PasswordAlgorithm {
    /// Argon2id, stored as a PHC string.
    #[sea_orm(string_value = "ARGON2ID")]
    Argon2id,
}

/// Rejected identity input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentityModelError {
    /// The value is not an Argon2id PHC string that fits the column.
    #[error("password hash is not a valid Argon2id PHC string")]
    PasswordHashFormat,
    /// The provider subject is empty or too long.
    #[error("provider subject is empty or too long")]
    ProviderSubjectLength,
    /// A tenant was supplied for a provider without tenants, or it is malformed.
    #[error("tenant id is only allowed for Microsoft identities and must be non-empty ASCII")]
    TenantNotAllowed,
}

/// An Argon2id password hash in PHC string format.
///
/// The value is secret: `Debug` prints `[REDACTED]`, there is no `Display` or
/// `Serialize` implementation, and the raw string is only reachable through
/// [`PasswordHash::expose`] for verification and persistence.
#[derive(Clone, PartialEq, Eq, DeriveValueType)]
pub struct PasswordHash(String);

impl PasswordHash {
    /// Wraps an Argon2id PHC string produced by the password hasher.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityModelError::PasswordHashFormat`] unless the value is
    /// ASCII, starts with `$argon2id$`, and fits the 255-byte column. The error
    /// never contains the input.
    pub fn new(phc: String) -> Result<Self, IdentityModelError> {
        if phc.is_ascii()
            && phc.len() <= MAX_PASSWORD_HASH_LEN
            && phc.len() > ARGON2ID_PHC_PREFIX.len()
            && phc.starts_with(ARGON2ID_PHC_PREFIX)
        {
            Ok(Self(phc))
        } else {
            Err(IdentityModelError::PasswordHashFormat)
        }
    }

    /// Returns the PHC string for verification or storage. Never log it.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PasswordHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasswordHash([REDACTED])")
    }
}

/// Insert model for public student signup.
///
/// There is deliberately no role or status field: the account is always a
/// [`Role::Student`] in the [`AccountStatus::Unverified`] state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewStudentAccount {
    email: String,
    normalized_email: String,
    display_name: String,
}

impl NewStudentAccount {
    /// Creates a signup record from already validated and normalized input.
    pub fn new(email: String, normalized_email: String, display_name: String) -> Self {
        Self {
            email,
            normalized_email,
            display_name,
        }
    }

    /// Builds the insert, generating a UUIDv7 and leaving timestamps to MySQL.
    pub fn into_active_model(self) -> user::ActiveModel {
        user::ActiveModel {
            id: Set(Uuid::now_v7()),
            email: Set(Some(self.email)),
            normalized_email: Set(Some(self.normalized_email)),
            display_name: Set(self.display_name),
            role: Set(Role::Student),
            account_status: Set(AccountStatus::Unverified),
            ..Default::default()
        }
    }
}

/// Insert model for a user's password credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewLocalCredential {
    user_id: Uuid,
    password_hash: PasswordHash,
}

impl NewLocalCredential {
    /// Creates a credential for `user_id`.
    pub fn new(user_id: Uuid, password_hash: PasswordHash) -> Self {
        Self {
            user_id,
            password_hash,
        }
    }

    /// Builds the insert, leaving timestamps to MySQL.
    pub fn into_active_model(self) -> local_credential::ActiveModel {
        local_credential::ActiveModel {
            user_id: Set(self.user_id),
            password_hash: Set(self.password_hash),
            password_algorithm: Set(PasswordAlgorithm::Argon2id),
            ..Default::default()
        }
    }
}

/// Insert model for linking a provider identity to a user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewExternalIdentity {
    user_id: Uuid,
    provider: IdentityProvider,
    provider_subject: String,
    tenant_id: Option<String>,
}

impl NewExternalIdentity {
    /// Creates a link from a verified ID token's `sub` (and Microsoft `tid`).
    ///
    /// # Errors
    ///
    /// Returns [`IdentityModelError::ProviderSubjectLength`] for an empty or
    /// over-long subject, and [`IdentityModelError::TenantNotAllowed`] when a
    /// tenant is given for a provider other than Microsoft or is malformed.
    pub fn new(
        user_id: Uuid,
        provider: IdentityProvider,
        provider_subject: String,
        tenant_id: Option<String>,
    ) -> Result<Self, IdentityModelError> {
        if provider_subject.is_empty()
            || provider_subject.chars().count() > MAX_PROVIDER_SUBJECT_LEN
        {
            return Err(IdentityModelError::ProviderSubjectLength);
        }
        if let Some(tenant) = &tenant_id {
            let well_formed =
                !tenant.is_empty() && tenant.is_ascii() && tenant.len() <= MAX_TENANT_ID_LEN;
            if provider != IdentityProvider::Microsoft || !well_formed {
                return Err(IdentityModelError::TenantNotAllowed);
            }
        }
        Ok(Self {
            user_id,
            provider,
            provider_subject,
            tenant_id,
        })
    }

    /// Builds the insert, generating a UUIDv7 and leaving `linked_at` to MySQL.
    pub fn into_active_model(self) -> external_identity::ActiveModel {
        external_identity::ActiveModel {
            id: Set(Uuid::now_v7()),
            user_id: Set(self.user_id),
            provider: Set(self.provider),
            provider_subject: Set(self.provider_subject),
            tenant_id: Set(self.tenant_id),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveEnum, ActiveValue, Iterable};

    use super::*;

    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNo";

    fn db_values<E: ActiveEnum<Value = String> + Iterable>() -> Vec<String> {
        E::iter().map(|variant| variant.to_value()).collect()
    }

    #[test]
    fn enum_values_match_the_migration_check_constraints() {
        assert_eq!(db_values::<Role>(), ["STUDENT", "ADMIN"]);
        assert_eq!(
            db_values::<AccountStatus>(),
            [
                "UNVERIFIED",
                "ACTIVE",
                "DISABLED",
                "PENDING_DELETION",
                "DELETED"
            ]
        );
        assert_eq!(db_values::<IdentityProvider>(), ["GOOGLE", "MICROSOFT"]);
        assert_eq!(db_values::<PasswordAlgorithm>(), ["ARGON2ID"]);
        assert_eq!(Role::try_from_value(&"ADMIN".to_owned()), Ok(Role::Admin));
        assert!(Role::try_from_value(&"admin".to_owned()).is_err());
    }

    #[test]
    fn password_hash_accepts_only_argon2id_phc_strings() {
        assert!(PasswordHash::new(HASH.to_owned()).is_ok());
        for rejected in [
            "",
            "$argon2id$",
            "correct horse battery staple",
            "$argon2i$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA",
            "$2b$12$abcdefghijklmnopqrstuv",
        ] {
            assert_eq!(
                PasswordHash::new(rejected.to_owned()),
                Err(IdentityModelError::PasswordHashFormat)
            );
        }
        let too_long = format!("{ARGON2ID_PHC_PREFIX}{}", "a".repeat(MAX_PASSWORD_HASH_LEN));
        assert!(PasswordHash::new(too_long).is_err());
    }

    #[test]
    fn password_hash_is_redacted_in_every_debug_representation() {
        let hash = PasswordHash::new(HASH.to_owned()).unwrap();
        assert_eq!(hash.expose(), HASH);

        let credential = NewLocalCredential::new(Uuid::now_v7(), hash.clone());
        let active = credential.clone().into_active_model();
        let model = local_credential::Model {
            user_id: Uuid::now_v7(),
            password_hash: hash.clone(),
            password_algorithm: PasswordAlgorithm::Argon2id,
            password_changed_at: DateTime::default(),
            created_at: DateTime::default(),
            updated_at: DateTime::default(),
        };

        for rendered in [
            format!("{hash:?}"),
            format!("{credential:?}"),
            format!("{active:?}"),
            format!("{model:?}"),
            format!("{:?}", IdentityModelError::PasswordHashFormat),
        ] {
            assert!(!rendered.contains("argon2id$v=19"), "leaked: {rendered}");
            assert!(!rendered.contains("aGFzaGhhc2hoYXNo"), "leaked: {rendered}");
        }
        assert!(format!("{active:?}").contains("[REDACTED]"));
    }

    #[test]
    fn local_credential_records_the_algorithm() {
        let active =
            NewLocalCredential::new(Uuid::now_v7(), PasswordHash::new(HASH.to_owned()).unwrap())
                .into_active_model();
        assert_eq!(active.password_algorithm, Set(PasswordAlgorithm::Argon2id));
    }

    #[test]
    fn student_signup_always_creates_an_unverified_student() {
        let active = NewStudentAccount::new(
            "Student@Example.com".to_owned(),
            "student@example.com".to_owned(),
            "Student".to_owned(),
        )
        .into_active_model();

        assert_eq!(active.role, Set(Role::Student));
        assert_eq!(active.account_status, Set(AccountStatus::Unverified));
        assert_eq!(
            active.normalized_email,
            Set(Some("student@example.com".to_owned()))
        );
        assert!(active.email_verified_at.is_not_set());
        assert!(active.deleted_at.is_not_set());
        let ActiveValue::Set(id) = active.id else {
            panic!("id must be generated");
        };
        assert_eq!(id.get_version_num(), 7);
    }

    #[test]
    fn tenants_are_only_allowed_for_microsoft() {
        let user = Uuid::now_v7();
        let tenant = Some("9188040d-6c67-4c5b-b112-36a304b66dad".to_owned());

        let microsoft = NewExternalIdentity::new(
            user,
            IdentityProvider::Microsoft,
            "sub".to_owned(),
            tenant.clone(),
        )
        .unwrap()
        .into_active_model();
        assert_eq!(microsoft.tenant_id, Set(tenant.clone()));
        assert!(
            NewExternalIdentity::new(user, IdentityProvider::Microsoft, "sub".to_owned(), None)
                .is_ok()
        );
        assert!(
            NewExternalIdentity::new(user, IdentityProvider::Google, "sub".to_owned(), None)
                .is_ok()
        );

        assert_eq!(
            NewExternalIdentity::new(user, IdentityProvider::Google, "sub".to_owned(), tenant),
            Err(IdentityModelError::TenantNotAllowed)
        );
        assert_eq!(
            NewExternalIdentity::new(
                user,
                IdentityProvider::Microsoft,
                "sub".to_owned(),
                Some(String::new())
            ),
            Err(IdentityModelError::TenantNotAllowed)
        );
    }

    #[test]
    fn provider_subjects_must_be_present_and_fit_the_column() {
        let user = Uuid::now_v7();
        assert_eq!(
            NewExternalIdentity::new(user, IdentityProvider::Google, String::new(), None),
            Err(IdentityModelError::ProviderSubjectLength)
        );
        assert_eq!(
            NewExternalIdentity::new(
                user,
                IdentityProvider::Google,
                "s".repeat(MAX_PROVIDER_SUBJECT_LEN + 1),
                None
            ),
            Err(IdentityModelError::ProviderSubjectLength)
        );
    }
}
