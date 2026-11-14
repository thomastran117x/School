//! Rate-limit policies: bucket size, refill rate and failure behaviour.

use crate::config::{BucketSettings, RateLimitSettings};

/// What to do when Redis cannot answer a limiter check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureMode {
    /// Reject with 503 and `Retry-After`. Used by every auth policy, so credential
    /// guessing is never allowed just because the shared store is down.
    Closed,
    /// Enforce a bounded per-process limit instead. Used by general traffic.
    Open,
}

/// One named token-bucket policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Fixed metric label and key-namespace separator.
    pub name: &'static str,
    /// Bucket size: the largest burst allowed.
    pub capacity: u32,
    /// Tokens added per minute while the bucket is not full.
    pub refill_per_minute: u32,
    /// Behaviour when Redis is unavailable.
    pub failure: FailureMode,
}

impl Policy {
    /// Builds a policy from validated bucket settings.
    fn new(name: &'static str, bucket: &BucketSettings, failure: FailureMode) -> Self {
        Self {
            name,
            capacity: bucket.capacity,
            refill_per_minute: bucket.refill_per_minute,
            failure,
        }
    }
}

/// The application's configured policies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policies {
    /// Applied to every request, keyed by client IP.
    pub general_ip: Policy,
    /// Applied to authentication routes, keyed by client IP. Always fails closed.
    pub auth_ip: Policy,
    /// Applied per account (for example normalised email) on authentication
    /// attempts. Always fails closed.
    pub auth_account: Policy,
}

impl Policies {
    /// Builds the policy set from settings. The failure mode of auth policies is not
    /// configurable.
    pub fn from_settings(settings: &RateLimitSettings) -> Self {
        Self {
            general_ip: Policy::new("general_ip", &settings.general_ip, FailureMode::Open),
            auth_ip: Policy::new("auth_ip", &settings.auth_ip, FailureMode::Closed),
            auth_account: Policy::new("auth_account", &settings.auth_account, FailureMode::Closed),
        }
    }
}
