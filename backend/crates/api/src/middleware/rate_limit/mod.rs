//! Distributed per-IP and per-account rate limiting.
//!
//! # Model
//!
//! Each subject (client IP or account) owns a token bucket in Redis, updated by one
//! atomic Lua script ([`script`]), so limits hold across every API replica. Buckets
//! allow a burst up to `capacity` and refill continuously; idle buckets expire.
//! Policies ([`policy`]) are `general_ip` for all traffic plus the stricter `auth_ip`
//! and `auth_account` for authentication.
//!
//! # Failure policy
//!
//! - **Auth policies fail closed.** If Redis cannot answer, the request gets 503 with
//!   `Retry-After`. Credential guessing is never allowed because the limiter is down.
//! - **The general policy degrades.** A bounded per-process table ([`fallback`])
//!   enforces a looser local limit and the outage is counted and logged.
//!
//! Redis failures and latency are exported as `rate_limiter_redis_errors_total` and
//! `rate_limiter_redis_duration_seconds`; decisions as `rate_limit_decisions_total`.
//!
//! # Client IP
//!
//! See [`client_ip`]. Forwarded headers are honoured only from `TRUSTED_PROXY_CIDRS`.
//!
//! # Using it
//!
//! The general policy is applied globally in [`crate::app`]. Authentication routes
//! add `route_layer(from_fn_with_state(limiter.ip_limit(policies.auth_ip), limit_by_ip))`
//! and call [`RateLimiter::check_account`] with `policies.auth_account` after the body
//! has been validated (extractors consume the body, so a layer cannot see the account).
// The auth policies are consumed once the authentication routes land (AUTH-07).
#![allow(dead_code)]

pub mod cidr;
pub mod client_ip;
pub mod fallback;
pub mod layer;
pub mod limiter;
pub mod policy;
pub mod script;

pub use layer::limit_by_ip;
pub use limiter::RateLimiter;
