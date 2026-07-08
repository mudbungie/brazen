//! Parse an HTTP `Retry-After` header (RFC 7231 §7.1.3) to a delay in WHOLE
//! seconds — the transport-level pacing hint a caller-owned retry loop wants
//! (architecture.md §3.3). Two wire forms: a bare `delay-seconds` integer, and an
//! `HTTP-date` (the preferred IMF-fixdate) whose delay is `date - now`, computed
//! against the injected clock's `now` (unix seconds) — NEVER a wall-clock read, so
//! the pure lib stays time-free and the parse is table-tested. `None` for an absent
//! / unparseable / malformed header (empty-set rule — never fabricated); a past date
//! yields `Some(0)` (retry now), the honest carry of a present-but-elapsed deadline.

/// Parse `header` (a `Retry-After` value) to whole seconds of delay relative to
/// `now` (unix seconds, from the [`Clock`](crate::store::Clock) seam). Integer form
/// is `now`-independent; the date form subtracts `now` (saturating at 0 for a past
/// date, clamped at `u32::MAX` for an absurdly distant one). `None` when it is
/// neither a non-negative integer nor a well-formed IMF-fixdate.
pub(crate) fn parse_retry_after(header: &str, now: u64) -> Option<u32> {
    let h = header.trim();
    // `delay-seconds`: a bare non-negative integer, `now`-independent.
    if let Ok(secs) = h.parse::<u32>() {
        return Some(secs);
    }
    // `HTTP-date`: the delay is the distance from `now` to the deadline.
    let target = parse_imf_fixdate(h)?;
    Some(target.saturating_sub(now).min(u32::MAX as u64) as u32)
}

/// Parse the preferred `HTTP-date` form — an IMF-fixdate, e.g.
/// `Sun, 06 Nov 1994 08:49:37 GMT` — to unix seconds. The two obsolete formats
/// (rfc850-date, asctime-date) are a documented narrowing: no live provider emits
/// them for `Retry-After`, and an unparsed value degrades to `None`, never a panic.
fn parse_imf_fixdate(s: &str) -> Option<u64> {
    let s = s.strip_suffix(" GMT")?;
    let (_day_name, rest) = s.split_once(", ")?; // drop "Sun"
    let mut parts = rest.split(' '); // DD Mon YYYY HH:MM:SS
    let day: u32 = parts.next()?.parse().ok()?;
    let month = month_num(parts.next()?)?;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut hms = parts.next()?.split(':');
    let hour: u64 = hms.next()?.parse().ok()?;
    let min: u64 = hms.next()?.parse().ok()?;
    let sec: u64 = hms.next()?.parse().ok()?;
    // Reject trailing junk or out-of-range time components.
    if parts.next().is_some() || hms.next().is_some() || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let secs = days
        .checked_mul(86_400)?
        .checked_add((hour * 3600 + min * 60 + sec) as i64)?;
    u64::try_from(secs).ok()
}

/// The three-letter English month abbreviation → 1-based month, else `None`.
fn month_num(mon: &str) -> Option<u32> {
    Some(match mon {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

/// Days since the unix epoch (1970-01-01) for a civil `y-m-d` (Howard Hinnant's
/// algorithm). The `y / 400` era is correct for `y >= 0`, and a `Retry-After` date
/// is always modern (well after year 1), so no negative-year branch is carried.
/// `day`/`month` are range-checked here (the one home), so a malformed date is
/// `None` rather than a silently-wrong timestamp.
fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let m = month as i64;
    let d = day as i64;
    let era = y / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    Some(era * 146097 + doe - 719468)
}

#[cfg(test)]
mod tests {
    use super::parse_retry_after;

    /// The integer `delay-seconds` form is `now`-independent and passes through.
    #[test]
    fn integer_seconds_pass_through_ignoring_now() {
        assert_eq!(parse_retry_after("120", 0), Some(120));
        assert_eq!(parse_retry_after("  30  ", 999), Some(30));
        assert_eq!(parse_retry_after("0", 5), Some(0));
    }

    /// The IMF-fixdate form is the deadline minus `now`: a future date is the
    /// positive distance, a past date saturates at 0, an absurd one clamps at MAX.
    #[test]
    fn http_date_is_deadline_minus_now() {
        // 1970-01-01 01:00:00 GMT = 3600 unix; Jan exercises the month<=2 branch.
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 01:00:00 GMT", 0),
            Some(3600)
        );
        // A later month exercises the month>2 branch; 1970-05-01 00:00:00 GMT.
        assert_eq!(
            parse_retry_after("Fri, 01 May 1970 00:00:00 GMT", 0),
            Some(10_368_000)
        );
        // now past the deadline → 0 (retry now), never a wraparound.
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:00:00 GMT", 100),
            Some(0)
        );
        // A distant year overflows u32 seconds → clamped, never truncated.
        assert_eq!(
            parse_retry_after("Fri, 31 Dec 9999 23:59:59 GMT", 0),
            Some(u32::MAX)
        );
    }

    /// Every malformed shape degrades to `None` — never a panic, never a fabrication.
    #[test]
    fn malformed_headers_are_none() {
        for h in [
            "",                                    // empty
            "soon",                                // neither int nor date
            "-5",                                  // negative int (not u32)
            "Thu, 01 Jan 1970 00:00:00",           // no " GMT" suffix
            "Thu 01 Jan 1970 00:00:00 GMT",        // no ", " after day name
            "Thu, 01 Foo 1970 00:00:00 GMT",       // bad month
            "Thu, 01 Jan 19XX 00:00:00 GMT",       // bad year
            "Thu, XX Jan 1970 00:00:00 GMT",       // bad day
            "Thu, 00 Jan 1970 00:00:00 GMT",       // day out of range
            "Thu, 01 Jan 1970 24:00:00 GMT",       // hour out of range
            "Thu, 01 Jan 1970 00:60:00 GMT",       // minute out of range
            "Thu, 01 Jan 1970 00:00 GMT",          // missing seconds field
            "Thu, 01 Jan 1970 00:00:00 extra GMT", // trailing junk before GMT
            "Thu, 01 Jan 1970 00:00:00:00 GMT",    // extra time field
        ] {
            assert_eq!(parse_retry_after(h, 0), None, "expected None for {h:?}");
        }
    }
}
