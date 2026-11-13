//! Lua scripts that make each refresh-session operation atomic.
//!
//! Scripts derive family keys from stored ids using a prefix argument. This is valid
//! because the deployment uses a single Redis node; a clustered deployment would need
//! hash tags to co-locate those keys.

/// Creates a family, its first token, and the user index entry.
///
/// KEYS: token, family, user set. ARGV: user id, family id, token TTL ms, family TTL ms.
pub(super) const CREATE: &str = r"
redis.call('HSET', KEYS[1], 'family', ARGV[2], 'user', ARGV[1], 'used', '0')
redis.call('PEXPIRE', KEYS[1], ARGV[3])
redis.call('SET', KEYS[2], ARGV[1], 'PX', ARGV[4])
redis.call('SADD', KEYS[3], ARGV[2])
if redis.call('PTTL', KEYS[3]) < tonumber(ARGV[4]) then
  redis.call('PEXPIRE', KEYS[3], ARGV[4])
end
return 1
";

/// Atomically spends a token and stores its successor.
///
/// KEYS: presented token, successor token. ARGV: family key prefix, token TTL ms.
/// Returns `{status}` or `{'rotated', user, family}`.
pub(super) const ROTATE: &str = r"
local record = redis.call('HMGET', KEYS[1], 'family', 'user', 'used')
if not record[1] then
  return {'invalid'}
end
local family_key = ARGV[1] .. record[1]
local family_ttl = redis.call('PTTL', family_key)
if family_ttl < 0 then
  return {'revoked'}
end
if record[3] == '1' then
  redis.call('DEL', family_key)
  return {'replay'}
end
redis.call('HSET', KEYS[1], 'used', '1')
local ttl = tonumber(ARGV[2])
if family_ttl < ttl then
  ttl = family_ttl
end
redis.call('HSET', KEYS[2], 'family', record[1], 'user', record[2], 'used', '0')
redis.call('PEXPIRE', KEYS[2], ttl)
return {'rotated', record[2], record[1], tostring(ttl)}
";

/// Revokes the family that owns the presented token.
///
/// KEYS: presented token. ARGV: family key prefix.
pub(super) const REVOKE_SESSION: &str = r"
local family = redis.call('HGET', KEYS[1], 'family')
if not family then
  return 0
end
return redis.call('DEL', ARGV[1] .. family)
";

/// Revokes every family indexed for a user.
///
/// KEYS: user set. ARGV: family key prefix.
pub(super) const REVOKE_USER: &str = r"
local families = redis.call('SMEMBERS', KEYS[1])
for _, family in ipairs(families) do
  redis.call('DEL', ARGV[1] .. family)
end
redis.call('DEL', KEYS[1])
return #families
";
