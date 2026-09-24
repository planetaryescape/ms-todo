/// Unix seconds as RFC 3339 in UTC, e.g. `2026-09-24T17:18:43Z`.
pub fn rfc3339(unix_seconds: i64) -> String {
    chrono::DateTime::from_timestamp(unix_seconds, 0)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| unix_seconds.to_string())
}
