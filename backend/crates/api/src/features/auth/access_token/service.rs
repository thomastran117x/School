//! Signing and verification of EdDSA (Ed25519) access tokens.
//!
//! Verification is pure computation over the configured public keys. The service
//! holds no datastore handle, so validating a token can never reach Redis or MySQL.

use std::{
    collections::HashMap,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, errors::ErrorKind};
use sea_orm::{ActiveEnum, prelude::Uuid};
use serde::Deserialize;

use super::models::{
    ACCESS_TOKEN_TYPE, AccessTokenClaims, AccessTokenError, AuthenticatedPrincipal, IssueError,
    IssuedAccessToken,
};
use crate::{
    config::{JwtSettings, MAX_ACCESS_TOKEN_TTL},
    features::auth::identity::Role,
};

/// Longest accepted compact token, in bytes. Real tokens are well under 1 KiB.
const MAX_TOKEN_LEN: usize = 4096;
/// The only accepted `alg` header value.
const ALGORITHM: &str = "EdDSA";

/// The protected-header fields this service interprets.
///
/// Parsing them before `jsonwebtoken` lets the policy be explicit and testable, and
/// serde rejects a header that repeats any of these fields.
#[derive(Deserialize)]
struct ProtectedHeader {
    /// Signing algorithm; must be exactly `EdDSA`.
    alg: Option<String>,
    /// Token type; must be `at+jwt`.
    typ: Option<String>,
    /// Key id; must name a configured key.
    kid: Option<String>,
    /// Critical extensions. None are supported, so any value is rejected.
    crit: Option<serde_json::Value>,
}

/// Issues and verifies stateless access tokens.
///
/// One instance is built at startup and shared through `AppState`.
pub struct AccessTokenService {
    /// Issued and required `iss`.
    issuer: String,
    /// Issued and required `aud`.
    audience: String,
    /// Lifetime of issued tokens.
    ttl: Duration,
    /// [`Self::ttl`] in whole seconds.
    ttl_seconds: i64,
    /// Clock-skew allowance in whole seconds.
    skew_seconds: i64,
    /// `kid` placed on issued tokens.
    signing_kid: String,
    /// Private signing key.
    encoding_key: EncodingKey,
    /// Public keys accepted for verification, by `kid`.
    decoding_keys: HashMap<String, DecodingKey>,
    /// `jsonwebtoken` validation pinned to EdDSA, issuer and audience.
    validation: Validation,
}

impl AccessTokenService {
    /// Builds the service from validated settings.
    ///
    /// The active signing key's public half is always accepted, plus every
    /// verification-only rotation key.
    pub fn new(settings: &JwtSettings) -> Self {
        let mut decoding_keys = HashMap::with_capacity(1 + settings.verification_keys.len());
        decoding_keys.insert(
            settings.signing_key.kid.clone(),
            DecodingKey::from_ed_der(&settings.signing_key.public_key),
        );
        for key in &settings.verification_keys {
            decoding_keys.insert(key.kid.clone(), DecodingKey::from_ed_der(&key.public_key));
        }

        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.algorithms = vec![Algorithm::EdDSA];
        // Time is checked by `check_claims` against an injectable clock.
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.leeway = 0;
        validation.set_issuer(&[&settings.issuer]);
        validation.set_audience(&[&settings.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        Self {
            issuer: settings.issuer.clone(),
            audience: settings.audience.clone(),
            ttl: settings.access_token_ttl,
            ttl_seconds: whole_seconds(settings.access_token_ttl),
            skew_seconds: whole_seconds(settings.clock_skew),
            signing_kid: settings.signing_key.kid.clone(),
            encoding_key: EncodingKey::from_ed_der(settings.signing_key.pkcs8_der()),
            decoding_keys,
            validation,
        }
    }

    /// Returns the key id placed on newly issued tokens.
    pub fn signing_key_id(&self) -> &str {
        &self.signing_kid
    }

    /// Returns the lifetime of newly issued tokens.
    pub fn token_ttl(&self) -> Duration {
        self.ttl
    }

    /// Issues an access token for `user_id` with `role`.
    ///
    /// The caller must take both values from the database, never from the request.
    ///
    /// # Errors
    ///
    /// Returns [`IssueError::Signing`] if signing fails; treat it as an internal error.
    pub fn issue(&self, user_id: Uuid, role: Role) -> Result<IssuedAccessToken, IssueError> {
        self.issue_at(user_id, role, unix_now())
    }

    /// Verifies a compact access token and returns the principal it authenticates.
    ///
    /// # Errors
    ///
    /// Returns an [`AccessTokenError`] naming the first failed check. Callers must
    /// map every variant to the same `401` response.
    pub fn verify(&self, token: &str) -> Result<AuthenticatedPrincipal, AccessTokenError> {
        self.verify_at(token, unix_now())
    }

    /// Issues a token as if the current time were `now` (Unix seconds).
    fn issue_at(
        &self,
        user_id: Uuid,
        role: Role,
        now: i64,
    ) -> Result<IssuedAccessToken, IssueError> {
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some(ACCESS_TOKEN_TYPE.to_owned());
        header.kid = Some(self.signing_kid.clone());
        let claims = AccessTokenClaims {
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            sub: user_id.to_string(),
            iat: now,
            exp: now.saturating_add(self.ttl_seconds),
            jti: Uuid::new_v4().to_string(),
            role: role.to_value(),
        };
        let token = jsonwebtoken::encode(&header, &claims, &self.encoding_key)
            .map_err(|_| IssueError::Signing)?;
        Ok(IssuedAccessToken {
            token,
            expires_in: self.ttl,
        })
    }

    /// Verifies a token as if the current time were `now` (Unix seconds).
    fn verify_at(&self, token: &str, now: i64) -> Result<AuthenticatedPrincipal, AccessTokenError> {
        let header = parse_header(token)?;
        if header.crit.is_some() {
            return Err(AccessTokenError::Malformed);
        }
        if header.alg.as_deref() != Some(ALGORITHM) {
            return Err(AccessTokenError::UnsupportedAlgorithm);
        }
        if !header.typ.as_deref().is_some_and(is_access_token_type) {
            return Err(AccessTokenError::WrongType);
        }
        let key = header
            .kid
            .as_deref()
            .and_then(|kid| self.decoding_keys.get(kid))
            .ok_or(AccessTokenError::UnknownKey)?;
        let claims = jsonwebtoken::decode::<AccessTokenClaims>(token, key, &self.validation)
            .map_err(map_decode_error)?
            .claims;
        self.check_claims(claims, now)
    }

    /// Applies the claim rules that `jsonwebtoken` does not enforce exactly.
    fn check_claims(
        &self,
        claims: AccessTokenClaims,
        now: i64,
    ) -> Result<AuthenticatedPrincipal, AccessTokenError> {
        if claims.iss != self.issuer {
            return Err(AccessTokenError::WrongIssuer);
        }
        if claims.aud != self.audience {
            return Err(AccessTokenError::WrongAudience);
        }
        let lifetime = claims
            .exp
            .checked_sub(claims.iat)
            .ok_or(AccessTokenError::InvalidClaims)?;
        if lifetime <= 0 || lifetime > whole_seconds(MAX_ACCESS_TOKEN_TTL) {
            return Err(AccessTokenError::InvalidClaims);
        }
        if now >= claims.exp.saturating_add(self.skew_seconds) {
            return Err(AccessTokenError::Expired);
        }
        if claims.iat > now.saturating_add(self.skew_seconds) {
            return Err(AccessTokenError::NotYetValid);
        }
        let user_id = Uuid::parse_str(&claims.sub)
            .ok()
            .filter(|id| !id.is_nil())
            .ok_or(AccessTokenError::InvalidClaims)?;
        let token_id = Uuid::parse_str(&claims.jti).map_err(|_| AccessTokenError::InvalidClaims)?;
        let role =
            Role::try_from_value(&claims.role).map_err(|_| AccessTokenError::InvalidClaims)?;
        Ok(AuthenticatedPrincipal::verified(
            user_id, role, token_id, claims.exp,
        ))
    }
}

impl fmt::Debug for AccessTokenService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut accepted: Vec<&str> = self.decoding_keys.keys().map(String::as_str).collect();
        accepted.sort_unstable();
        formatter
            .debug_struct("AccessTokenService")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("ttl", &self.ttl)
            .field("signing_kid", &self.signing_kid)
            .field("accepted_kids", &accepted)
            .finish_non_exhaustive()
    }
}

/// Checks the compact shape and decodes the protected header.
fn parse_header(token: &str) -> Result<ProtectedHeader, AccessTokenError> {
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return Err(AccessTokenError::Malformed);
    }
    let mut segments = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(AccessTokenError::Malformed);
    };
    // An empty signature is left for the algorithm check, so `alg: none` is reported
    // as an unsupported algorithm rather than as a shape error.
    if header.is_empty()
        || payload.is_empty()
        || ![header, payload, signature]
            .iter()
            .all(|segment| is_base64url(segment))
    {
        return Err(AccessTokenError::Malformed);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(header)
        .map_err(|_| AccessTokenError::Malformed)?;
    serde_json::from_slice(&bytes).map_err(|_| AccessTokenError::Malformed)
}

/// Returns whether `segment` uses only the unpadded base64url alphabet.
fn is_base64url(segment: &str) -> bool {
    segment
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Returns whether a `typ` header names an access token (case-insensitive, RFC 9068).
fn is_access_token_type(typ: &str) -> bool {
    typ.eq_ignore_ascii_case(ACCESS_TOKEN_TYPE) || typ.eq_ignore_ascii_case("application/at+jwt")
}

/// Maps a `jsonwebtoken` failure to the service's error, discarding its details.
fn map_decode_error(error: jsonwebtoken::errors::Error) -> AccessTokenError {
    match error.kind() {
        ErrorKind::InvalidSignature | ErrorKind::InvalidEddsaKey | ErrorKind::InvalidKeyFormat => {
            AccessTokenError::InvalidSignature
        }
        ErrorKind::InvalidAlgorithm
        | ErrorKind::UnsupportedAlgorithm
        | ErrorKind::InvalidAlgorithmName => AccessTokenError::UnsupportedAlgorithm,
        ErrorKind::InvalidIssuer => AccessTokenError::WrongIssuer,
        ErrorKind::InvalidAudience => AccessTokenError::WrongAudience,
        ErrorKind::ExpiredSignature => AccessTokenError::Expired,
        ErrorKind::ImmatureSignature => AccessTokenError::NotYetValid,
        ErrorKind::MissingRequiredClaim(_)
        | ErrorKind::InvalidClaimFormat(_)
        | ErrorKind::InvalidSubject
        | ErrorKind::Json(_) => AccessTokenError::InvalidClaims,
        _ => AccessTokenError::Malformed,
    }
}

/// Converts a duration to whole seconds as `i64`, saturating.
fn whole_seconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

/// Returns the current Unix time in seconds.
///
/// A clock before the epoch yields `0`, which fails closed: issued tokens are already
/// expired and every presented token looks expired.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, whole_seconds)
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD;
    use ed25519_dalek::{Signer, SigningKey, pkcs8::DecodePrivateKey};
    use serde_json::{Value, json};

    use super::*;
    use crate::features::auth::access_token::test_keys::{
        PRIMARY_KID, PRIMARY_PRIVATE, PRIMARY_PUBLIC, SECONDARY_KID, SECONDARY_PRIVATE,
    };

    const NOW: i64 = 1_800_000_000;
    const USER: &str = "0190b6f2-7c1e-7a2b-8c3d-4e5f60718293";
    const JTI: &str = "5b1f6c1e-3a0e-4f53-9a43-2f8c0e3d9b11";

    fn settings(extra: &[(&str, &str)]) -> JwtSettings {
        let mut values = vec![
            ("JWT_SIGNING_KEY_ID", PRIMARY_KID),
            ("JWT_SIGNING_KEY", PRIMARY_PRIVATE),
        ];
        values.extend_from_slice(extra);
        JwtSettings::from_pairs(&values).unwrap()
    }

    fn service() -> AccessTokenService {
        AccessTokenService::new(&settings(&[]))
    }

    fn user() -> Uuid {
        Uuid::parse_str(USER).unwrap()
    }

    fn header() -> Value {
        json!({"alg": "EdDSA", "typ": "at+jwt", "kid": PRIMARY_KID})
    }

    fn claims() -> Value {
        json!({
            "iss": "school-api",
            "aud": "school-api",
            "sub": USER,
            "iat": NOW,
            "exp": NOW + 900,
            "jti": JTI,
            "role": "STUDENT",
        })
    }

    /// Returns `value` with `field` replaced, or removed when `new` is `None`.
    fn with(mut value: Value, field: &str, new: Option<Value>) -> Value {
        let object = value.as_object_mut().unwrap();
        match new {
            Some(new) => object.insert(field.to_owned(), new),
            None => object.remove(field),
        };
        value
    }

    fn b64(value: &Value) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    }

    /// Signs already-encoded segments with an Ed25519 key, bypassing the service.
    fn sign_segments(header: &str, payload: &str, private_key: &str) -> String {
        let key = SigningKey::from_pkcs8_der(&STANDARD.decode(private_key).unwrap()).unwrap();
        let input = format!("{header}.{payload}");
        let signature = key.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    /// Signs an arbitrary header and claims with the primary test key.
    fn sign(header: &Value, claims: &Value) -> String {
        sign_segments(&b64(header), &b64(claims), PRIMARY_PRIVATE)
    }

    fn decode_segment(segment: &str) -> Value {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segment).unwrap()).unwrap()
    }

    fn verify(token: &str) -> Result<AuthenticatedPrincipal, AccessTokenError> {
        service().verify_at(token, NOW)
    }

    #[test]
    fn issues_tokens_with_the_documented_header_and_claims() {
        let issued = service().issue_at(user(), Role::Student, NOW).unwrap();
        assert_eq!(issued.expires_in, Duration::from_secs(900));

        let segments: Vec<&str> = issued.token.split('.').collect();
        assert_eq!(segments.len(), 3);
        assert_eq!(decode_segment(segments[0]), header());

        let payload = decode_segment(segments[1]);
        let mut names: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["aud", "exp", "iat", "iss", "jti", "role", "sub"]);
        assert_eq!(payload["iss"], "school-api");
        assert_eq!(payload["aud"], "school-api");
        assert_eq!(payload["sub"], USER);
        assert_eq!(payload["iat"], NOW);
        assert_eq!(payload["exp"], NOW + 900);
        assert_eq!(payload["role"], "STUDENT");
        let jti = Uuid::parse_str(payload["jti"].as_str().unwrap()).unwrap();
        assert_eq!(jti.get_version_num(), 4);
    }

    #[test]
    fn a_correctly_signed_token_yields_a_trusted_principal() {
        let service = service();
        let issued = service.issue_at(user(), Role::Admin, NOW).unwrap();
        let principal = service.verify_at(&issued.token, NOW + 1).unwrap();

        assert_eq!(principal.user_id(), user());
        assert_eq!(principal.role(), Role::Admin);
        assert_eq!(principal.expires_at(), NOW + 900);
        let jti = decode_segment(issued.token.split('.').nth(1).unwrap())["jti"].clone();
        assert_eq!(principal.token_id().to_string(), jti.as_str().unwrap());

        // The hand-signing helper produces tokens the service accepts.
        assert_eq!(
            verify(&sign(&header(), &claims())).unwrap().user_id(),
            user()
        );
        // Each token gets a fresh id.
        let again = service.issue_at(user(), Role::Admin, NOW).unwrap();
        assert_ne!(issued.token, again.token);
    }

    #[test]
    fn issued_tokens_verify_against_the_real_clock() {
        let service = service();
        let issued = service.issue(user(), Role::Student).unwrap();
        assert_eq!(service.verify(&issued.token).unwrap().user_id(), user());
    }

    #[test]
    fn tampered_tokens_are_rejected() {
        let token = service()
            .issue_at(user(), Role::Student, NOW)
            .unwrap()
            .token;
        let segments: Vec<&str> = token.split('.').collect();

        let escalated = b64(&with(
            decode_segment(segments[1]),
            "role",
            Some(json!("ADMIN")),
        ));
        let forged_payload = format!("{}.{escalated}.{}", segments[0], segments[2]);
        assert_eq!(
            verify(&forged_payload),
            Err(AccessTokenError::InvalidSignature)
        );

        let mut signature = segments[2].to_owned();
        let first = if signature.starts_with('A') { "B" } else { "A" };
        signature.replace_range(0..1, first);
        let forged_signature = format!("{}.{}.{signature}", segments[0], segments[1]);
        assert_eq!(
            verify(&forged_signature),
            Err(AccessTokenError::InvalidSignature)
        );

        // Signed by a different key that claims the trusted key id.
        let impostor = sign_segments(&b64(&header()), &b64(&claims()), SECONDARY_PRIVATE);
        assert_eq!(verify(&impostor), Err(AccessTokenError::InvalidSignature));
    }

    #[test]
    fn unknown_or_missing_key_ids_are_rejected() {
        let unknown = with(header(), "kid", Some(json!("retired")));
        assert_eq!(
            verify(&sign(&unknown, &claims())),
            Err(AccessTokenError::UnknownKey)
        );
        let missing = with(header(), "kid", None);
        assert_eq!(
            verify(&sign(&missing, &claims())),
            Err(AccessTokenError::UnknownKey)
        );
    }

    #[test]
    fn expiry_and_issue_time_honour_the_clock_skew() {
        let service = service();
        let token = service.issue_at(user(), Role::Student, NOW).unwrap().token;
        assert!(service.verify_at(&token, NOW + 900 + 29).is_ok());
        assert_eq!(
            service.verify_at(&token, NOW + 900 + 30),
            Err(AccessTokenError::Expired)
        );

        let future = service
            .issue_at(user(), Role::Student, NOW + 30)
            .unwrap()
            .token;
        assert!(service.verify_at(&future, NOW).is_ok());
        let far_future = service
            .issue_at(user(), Role::Student, NOW + 31)
            .unwrap()
            .token;
        assert_eq!(
            service.verify_at(&far_future, NOW),
            Err(AccessTokenError::NotYetValid)
        );

        let strict = AccessTokenService::new(&settings(&[("JWT_CLOCK_SKEW_SECONDS", "0")]));
        let token = strict.issue_at(user(), Role::Student, NOW).unwrap().token;
        assert_eq!(
            strict.verify_at(&token, NOW + 900),
            Err(AccessTokenError::Expired)
        );
    }

    #[test]
    fn lifetimes_outside_the_allowed_range_are_rejected() {
        for exp in [NOW + 3601, NOW, NOW - 1] {
            let token = sign(&header(), &with(claims(), "exp", Some(json!(exp))));
            assert_eq!(
                verify(&token),
                Err(AccessTokenError::InvalidClaims),
                "{exp}"
            );
        }
        let longest = sign(&header(), &with(claims(), "exp", Some(json!(NOW + 3600))));
        assert!(verify(&longest).is_ok());
    }

    #[test]
    fn wrong_issuer_or_audience_is_rejected() {
        let issuer = with(claims(), "iss", Some(json!("https://evil.example")));
        assert_eq!(
            verify(&sign(&header(), &issuer)),
            Err(AccessTokenError::WrongIssuer)
        );
        let audience = with(claims(), "aud", Some(json!("another-api")));
        assert_eq!(
            verify(&sign(&header(), &audience)),
            Err(AccessTokenError::WrongAudience)
        );

        // A service configured for another audience rejects this service's tokens.
        let token = service()
            .issue_at(user(), Role::Student, NOW)
            .unwrap()
            .token;
        let other = AccessTokenService::new(&settings(&[("JWT_AUDIENCE", "reports-api")]));
        assert_eq!(
            other.verify_at(&token, NOW),
            Err(AccessTokenError::WrongAudience)
        );
    }

    #[test]
    fn claims_of_the_wrong_json_type_are_rejected() {
        for (field, value) in [
            ("aud", json!(["school-api"])),
            ("iss", json!(["school-api"])),
            ("sub", json!(7)),
            ("iat", json!(1.5)),
            ("exp", json!("tomorrow")),
        ] {
            let token = sign(&header(), &with(claims(), field, Some(value)));
            assert!(verify(&token).is_err(), "{field}");
        }
    }

    #[test]
    fn unsupported_algorithms_are_rejected() {
        let unsigned = format!(
            "{}.{}.",
            b64(&with(header(), "alg", Some(json!("none")))),
            b64(&claims())
        );
        assert_eq!(
            verify(&unsigned),
            Err(AccessTokenError::UnsupportedAlgorithm)
        );

        for alg in ["none", "RS256", "ES256", "HS256", "eddsa", "Ed25519"] {
            let token = sign(&with(header(), "alg", Some(json!(alg))), &claims());
            assert_eq!(
                verify(&token),
                Err(AccessTokenError::UnsupportedAlgorithm),
                "{alg}"
            );
        }

        // Classic key confusion: HMAC "signed" with the public verification key.
        let public = STANDARD.decode(PRIMARY_PUBLIC).unwrap();
        let mut hs256 = Header::new(Algorithm::HS256);
        hs256.typ = Some("at+jwt".to_owned());
        hs256.kid = Some(PRIMARY_KID.to_owned());
        let confused =
            jsonwebtoken::encode(&hs256, &claims(), &EncodingKey::from_secret(&public)).unwrap();
        assert_eq!(
            verify(&confused),
            Err(AccessTokenError::UnsupportedAlgorithm)
        );
        let raw_public = &public[public.len() - 32..];
        let confused =
            jsonwebtoken::encode(&hs256, &claims(), &EncodingKey::from_secret(raw_public)).unwrap();
        assert_eq!(
            verify(&confused),
            Err(AccessTokenError::UnsupportedAlgorithm)
        );
    }

    #[test]
    fn only_access_token_types_are_accepted() {
        for typ in [Some(json!("JWT")), Some(json!("refresh+jwt")), None] {
            let token = sign(&with(header(), "typ", typ.clone()), &claims());
            assert_eq!(verify(&token), Err(AccessTokenError::WrongType), "{typ:?}");
        }
        for typ in ["AT+JWT", "application/at+jwt"] {
            let token = sign(&with(header(), "typ", Some(json!(typ))), &claims());
            assert!(verify(&token).is_ok(), "{typ}");
        }
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        let valid = sign(&header(), &claims());
        let refresh_token = URL_SAFE_NO_PAD.encode([7_u8; 32]);
        let not_json = format!("{}.{}.", URL_SAFE_NO_PAD.encode("nope"), b64(&claims()));
        let padded = format!("{valid}=");
        let oversized = format!("{}.{}.sig", b64(&header()), "a".repeat(MAX_TOKEN_LEN));
        for token in [
            "",
            "abc",
            "a.b",
            "a.b.c.d",
            ".b.c",
            "a..c",
            "!!!.###.$$$",
            "a b.c d.e f",
            refresh_token.as_str(),
            not_json.as_str(),
            padded.as_str(),
            oversized.as_str(),
        ] {
            assert_eq!(verify(token), Err(AccessTokenError::Malformed), "{token}");
        }

        let duplicate = r#"{"alg":"none","alg":"EdDSA","typ":"at+jwt","kid":"test-primary"}"#;
        let token = sign_segments(
            &URL_SAFE_NO_PAD.encode(duplicate),
            &b64(&claims()),
            PRIMARY_PRIVATE,
        );
        assert_eq!(verify(&token), Err(AccessTokenError::Malformed));

        let critical = with(header(), "crit", Some(json!(["exp"])));
        assert_eq!(
            verify(&sign(&critical, &claims())),
            Err(AccessTokenError::Malformed)
        );
    }

    #[test]
    fn signed_payloads_that_are_not_valid_claims_are_rejected() {
        let not_json = sign_segments(
            &b64(&header()),
            &URL_SAFE_NO_PAD.encode("not json"),
            PRIMARY_PRIVATE,
        );
        assert_eq!(verify(&not_json), Err(AccessTokenError::InvalidClaims));

        for field in ["iss", "aud", "sub", "iat", "exp", "jti", "role"] {
            let token = sign(&header(), &with(claims(), field, None));
            assert_eq!(
                verify(&token),
                Err(AccessTokenError::InvalidClaims),
                "missing {field}"
            );
        }
        for (field, value) in [
            ("sub", "not-a-uuid"),
            ("sub", "00000000-0000-0000-0000-000000000000"),
            ("jti", "not-a-uuid"),
            ("role", "SUPERADMIN"),
            ("role", "admin"),
            ("role", ""),
        ] {
            let token = sign(&header(), &with(claims(), field, Some(json!(value))));
            assert_eq!(
                verify(&token),
                Err(AccessTokenError::InvalidClaims),
                "{field}={value}"
            );
        }
    }

    #[test]
    fn rotation_keeps_old_tokens_valid_until_their_key_is_removed() {
        let old = service();
        let old_token = old.issue_at(user(), Role::Student, NOW).unwrap().token;

        let previous = format!("{PRIMARY_KID}:{PRIMARY_PUBLIC}");
        let rotated = AccessTokenService::new(&settings(&[
            ("JWT_SIGNING_KEY_ID", SECONDARY_KID),
            ("JWT_SIGNING_KEY", SECONDARY_PRIVATE),
            ("JWT_VERIFICATION_KEYS", &previous),
        ]));
        assert_eq!(rotated.signing_key_id(), SECONDARY_KID);
        assert!(rotated.verify_at(&old_token, NOW).is_ok());

        let new_token = rotated.issue_at(user(), Role::Student, NOW).unwrap().token;
        assert_eq!(
            decode_segment(new_token.split('.').next().unwrap())["kid"],
            SECONDARY_KID
        );
        assert!(rotated.verify_at(&new_token, NOW).is_ok());
        // A replica that has not been given the new public key rejects new tokens.
        assert_eq!(
            old.verify_at(&new_token, NOW),
            Err(AccessTokenError::UnknownKey)
        );

        let retired = AccessTokenService::new(&settings(&[
            ("JWT_SIGNING_KEY_ID", SECONDARY_KID),
            ("JWT_SIGNING_KEY", SECONDARY_PRIVATE),
        ]));
        assert_eq!(
            retired.verify_at(&old_token, NOW),
            Err(AccessTokenError::UnknownKey)
        );
    }

    #[test]
    fn debug_output_never_contains_tokens_or_keys() {
        let service = service();
        let issued = service.issue_at(user(), Role::Student, NOW).unwrap();
        let jwt_settings = settings(&[]);
        let public_b64url = URL_SAFE_NO_PAD.encode(jwt_settings.signing_key.public_key);
        for rendered in [
            format!("{issued:?}"),
            format!("{service:?}"),
            format!("{jwt_settings:?}"),
        ] {
            assert!(!rendered.contains(&issued.token), "{rendered}");
            assert!(!rendered.contains(issued.token.split('.').nth(1).unwrap()));
            assert!(!rendered.contains(PRIMARY_PRIVATE), "{rendered}");
            assert!(!rendered.contains(PRIMARY_PUBLIC), "{rendered}");
            assert!(!rendered.contains(&public_b64url), "{rendered}");
        }
        assert!(format!("{issued:?}").contains("<redacted>"));
    }

    #[test]
    fn error_labels_are_distinct() {
        let labels = [
            AccessTokenError::MissingCredentials,
            AccessTokenError::Malformed,
            AccessTokenError::UnsupportedAlgorithm,
            AccessTokenError::WrongType,
            AccessTokenError::UnknownKey,
            AccessTokenError::InvalidSignature,
            AccessTokenError::Expired,
            AccessTokenError::NotYetValid,
            AccessTokenError::WrongIssuer,
            AccessTokenError::WrongAudience,
            AccessTokenError::InvalidClaims,
        ]
        .map(AccessTokenError::label);
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len());
    }
}
