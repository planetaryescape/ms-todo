// Adapted from spotuify crates/spotuify-spotify/src/rate_limit.rs
// (`decide_retry`, `jittered_backoff`) @ d807e5e4f9d2f09878cdc22309af3589623f7785.
// Changes: Graph's rules from docs/blueprint/03-graph-provider.md#http-client
// (408 is transient, 401 refreshes once, nothing but 429 and 401 is retried
// for a non-idempotent request), and the decision no longer builds the error,
// so it stays free of randomness and I/O. The persisted per-scope backoff and
// priority lanes are left for the sync loop and outbox (rungs 3a and 4).

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};

/// Retries after a 429, 5xx, 408 or network failure, in total, per request.
pub const MAX_RETRIES: u32 = 3;

/// The longest `Retry-After` we wait inside one request. Anything longer
/// comes back as `RateLimited` with the wait, so a CLI call never looks hung.
pub const MAX_IN_REQUEST_RETRY_AFTER: Duration = Duration::from_secs(30);

const BACKOFF_BASE: Duration = Duration::from_millis(500);
const BACKOFF_CEILING: Duration = Duration::from_secs(30);

/// What to do with a response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryDecision {
    /// 2xx: use the response.
    Success,
    /// 429 with a usable `Retry-After`: wait exactly this long, then resend.
    RetryAfter(Duration),
    /// Wait [`jittered_backoff`] for this attempt, then resend.
    Backoff,
    /// 401: refresh the token, then resend. The caller allows this once.
    RefreshToken,
    /// Stop and return the error for this status.
    GiveUp,
}

/// Decide what to do after a response. `attempt` counts the retries already
/// made for this request (0 after the first send). `idempotent` is true for
/// GETs, absolute PATCHes and DELETEs; a request that isn't is only resent
/// when the response proves it didn't run (D-028).
pub fn decide_retry(
    status: StatusCode,
    headers: &HeaderMap,
    attempt: u32,
    idempotent: bool,
) -> RetryDecision {
    if status.is_success() {
        return RetryDecision::Success;
    }
    if status == StatusCode::UNAUTHORIZED {
        return RetryDecision::RefreshToken;
    }
    if attempt >= MAX_RETRIES {
        return RetryDecision::GiveUp;
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        // A 429 means Graph didn't run the request, so it's always safe to resend.
        return match retry_after(headers) {
            Some(wait) if wait > MAX_IN_REQUEST_RETRY_AFTER => RetryDecision::GiveUp,
            Some(wait) => RetryDecision::RetryAfter(wait),
            None => RetryDecision::Backoff,
        };
    }
    let transient = status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT;
    if transient && idempotent {
        RetryDecision::Backoff
    } else {
        RetryDecision::GiveUp
    }
}

/// Whether to resend after the request failed without a response.
pub fn decide_network_retry(error: &reqwest::Error, attempt: u32, idempotent: bool) -> bool {
    // A failed connect never reached Graph, so even a create is safe to resend.
    attempt < MAX_RETRIES && (idempotent || error.is_connect())
}

/// `Retry-After` as a wait: delay seconds (what To Do sends, S9) or an
/// HTTP date. A date in the past is no wait at all.
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    let wait = at.signed_duration_since(chrono::Utc::now());
    Some(wait.to_std().unwrap_or(Duration::ZERO))
}

/// 500 ms, 1 s, 2 s, … for attempts 0, 1, 2, …, each ±25%, capped at 30 s.
/// `unit` scales the base so tests don't wait real seconds.
pub fn jittered_backoff(attempt: u32, unit: Duration) -> Duration {
    let base = BACKOFF_BASE
        .mul_f64(unit.as_secs_f64())
        .saturating_mul(1 << attempt.min(10))
        .min(BACKOFF_CEILING);
    base.mul_f64(0.75 + fastrand::f64() * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(retry_after: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = retry_after {
            headers.insert(RETRY_AFTER, HeaderValue::from_str(value).expect("header"));
        }
        headers
    }

    fn decide(
        status: u16,
        retry_after: Option<&str>,
        attempt: u32,
        idempotent: bool,
    ) -> RetryDecision {
        let status = StatusCode::from_u16(status).expect("status");
        decide_retry(status, &headers(retry_after), attempt, idempotent)
    }

    #[test]
    fn success_is_used() {
        assert_eq!(decide(200, None, 0, true), RetryDecision::Success);
        assert_eq!(decide(204, None, 3, false), RetryDecision::Success);
    }

    #[test]
    fn a_429_honours_retry_after_in_seconds() {
        assert_eq!(
            decide(429, Some("7"), 0, true),
            RetryDecision::RetryAfter(Duration::from_secs(7))
        );
    }

    #[test]
    fn a_429_is_resent_even_when_not_idempotent() {
        assert_eq!(
            decide(429, Some("1"), 0, false),
            RetryDecision::RetryAfter(Duration::from_secs(1))
        );
    }

    #[test]
    fn a_429_without_retry_after_backs_off() {
        assert_eq!(decide(429, None, 1, true), RetryDecision::Backoff);
        assert_eq!(decide(429, Some("soon"), 1, true), RetryDecision::Backoff);
    }

    #[test]
    fn a_429_with_a_long_wait_gives_up_instead_of_hanging() {
        assert_eq!(decide(429, Some("120"), 0, true), RetryDecision::GiveUp);
    }

    #[test]
    fn a_429_accepts_an_http_date() {
        let past = "Wed, 21 Oct 2015 07:28:00 GMT";
        assert_eq!(
            decide(429, Some(past), 0, true),
            RetryDecision::RetryAfter(Duration::ZERO)
        );
        let future = (chrono::Utc::now() + chrono::Duration::seconds(10)).to_rfc2822();
        let wait = match decide(429, Some(&future), 0, true) {
            RetryDecision::RetryAfter(wait) => Some(wait),
            _ => None,
        }
        .expect("RetryAfter");
        assert!(
            wait <= Duration::from_secs(10) && wait >= Duration::from_secs(8),
            "{wait:?}"
        );
    }

    #[test]
    fn server_errors_and_408_back_off_for_idempotent_requests_only() {
        for status in [500, 502, 503, 504, 408] {
            assert_eq!(
                decide(status, None, 0, true),
                RetryDecision::Backoff,
                "{status}"
            );
            assert_eq!(
                decide(status, None, 0, false),
                RetryDecision::GiveUp,
                "{status}"
            );
        }
    }

    #[test]
    fn retries_stop_after_three() {
        assert_eq!(decide(503, None, 2, true), RetryDecision::Backoff);
        assert_eq!(decide(503, None, 3, true), RetryDecision::GiveUp);
        assert_eq!(decide(429, Some("1"), 3, true), RetryDecision::GiveUp);
    }

    #[test]
    fn a_401_asks_for_a_refresh() {
        assert_eq!(decide(401, None, 0, true), RetryDecision::RefreshToken);
        assert_eq!(decide(401, None, 0, false), RetryDecision::RefreshToken);
    }

    #[test]
    fn other_client_errors_are_final() {
        for status in [400, 403, 404, 409, 412] {
            assert_eq!(
                decide(status, None, 0, true),
                RetryDecision::GiveUp,
                "{status}"
            );
        }
    }

    #[test]
    fn backoff_doubles_with_jitter_and_is_capped() {
        let unit = Duration::from_secs(1);
        for (attempt, base_ms) in [(0, 500.0), (1, 1000.0), (2, 2000.0)] {
            let wait = jittered_backoff(attempt, unit).as_secs_f64() * 1000.0;
            assert!(
                wait >= base_ms * 0.75 && wait <= base_ms * 1.25,
                "{attempt}: {wait}"
            );
        }
        assert!(jittered_backoff(30, unit) <= BACKOFF_CEILING.mul_f64(1.25));
    }
}
