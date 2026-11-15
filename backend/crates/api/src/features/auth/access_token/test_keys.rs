//! Fixed Ed25519 key pairs for tests.
//!
//! These keys are public test fixtures. Never configure them outside tests.
//! Private keys are base64 PKCS#8 DER, public keys are base64 SPKI DER, which is the
//! same encoding the `JWT_SIGNING_KEY` and `JWT_VERIFICATION_KEYS` variables use.

/// Key id of the default test signing key.
pub(crate) const PRIMARY_KID: &str = "test-primary";
/// Private key of the default test signing key.
pub(crate) const PRIMARY_PRIVATE: &str =
    "MC4CAQAwBQYDK2VwBCIEIOTg18nForUnB/sPZ9TEXP4JFns+uKYA4/qsOg2DIE0y";
/// Public key of the default test signing key.
pub(crate) const PRIMARY_PUBLIC: &str =
    "MCowBQYDK2VwAyEAkTbxGpzDzVF1G6OevqgRBkwYFslSXu5kOXGqOI71D1g=";

/// Key id of the second test key, used for rotation and wrong-key cases.
pub(crate) const SECONDARY_KID: &str = "test-secondary";
/// Private key of the second test key.
pub(crate) const SECONDARY_PRIVATE: &str =
    "MC4CAQAwBQYDK2VwBCIEIEzk8FC9ofRe+suw7+6Z5UyvD2+RpjXiIUa26Q6QYAO5";
/// Public key of the second test key.
pub(crate) const SECONDARY_PUBLIC: &str =
    "MCowBQYDK2VwAyEAB7NSclvti4o4ci30BJ6YRMXK8qYG1Wmg+QzuhSxycrQ=";

/// The development key published in `.env.example` and `compose.yaml`.
pub(crate) const DEV_PRIVATE: &str =
    "MC4CAQAwBQYDK2VwBCIEIGIaIVbe31oMpSpWpZO1y3LHMthkEcRk+YHg/hvHcyWU";
/// Public half of [`DEV_PRIVATE`].
pub(crate) const DEV_PUBLIC: &str = "MCowBQYDK2VwAyEABlnnC5YI2RAVE3BjHp8RkTSCd4Pz7kvPO1vdRZx9k28=";
