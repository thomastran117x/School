//! Lua script implementing the shared token bucket.

/// Atomic token-bucket check-and-consume.
///
/// `KEYS[1]`: the bucket key (a hash holding `tokens` and `ts`).
///
/// `ARGV[1]`: bucket capacity (maximum burst).
/// `ARGV[2]`: tokens refilled per minute.
///
/// Time comes from Redis (`TIME`), not from the application, so replicas with skewed
/// clocks agree. Returns `{allowed, remaining_tokens, retry_after_ms}`; `retry_after_ms`
/// is `0` when allowed. The key expires once the bucket would be full again, so idle
/// subjects leave nothing behind.
pub(super) const TOKEN_BUCKET: &str = r"
local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)
local capacity = tonumber(ARGV[1])
local per_minute = tonumber(ARGV[2])

local state = redis.call('HMGET', KEYS[1], 'tokens', 'ts')
local tokens = tonumber(state[1])
local ts = tonumber(state[2])
if tokens == nil or ts == nil then
  tokens = capacity
  ts = now
end

local elapsed = now - ts
if elapsed < 0 then elapsed = 0 end
tokens = math.min(capacity, tokens + elapsed * per_minute / 60000)

local allowed = 0
local retry_ms = 0
if tokens >= 1 then
  tokens = tokens - 1
  allowed = 1
else
  retry_ms = math.ceil((1 - tokens) * 60000 / per_minute)
end

redis.call('HSET', KEYS[1], 'tokens', tostring(tokens), 'ts', tostring(now))
redis.call('PEXPIRE', KEYS[1], math.ceil((capacity - tokens) * 60000 / per_minute) + 1000)
return {allowed, math.floor(tokens), retry_ms}
";
