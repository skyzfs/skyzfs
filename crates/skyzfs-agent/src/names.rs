//! The names the agent gives its snapshots (the plan of M2.6, §6, and
//! the lead's Q6): `skz-YYYYMMDDTHHMMSSZ`, the second (UTC) in which the
//! agent names it, and `-N` after it where that name is taken already.
//!
//! A name is one the protocol takes ([`skyzfs_control::valid_name`]) and
//! one ZFS takes after `@`: no `:` and no space, which ISO 8601's basic
//! form allows. Names of one length sort as their times do, for years
//! 1970 to 9999.

/// What every name of the agent begins with.
pub const PREFIX: &str = "skz-";

/// The name for the second `unix_s` (seconds since 1970, UTC), with
/// `-taken` after it where `taken` is not zero.
pub fn name(unix_s: u64, taken: u32) -> String {
    let (year, month, day) = civil(unix_s / 86_400);
    let second = unix_s % 86_400;
    let mut name = format!(
        "{PREFIX}{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        second / 3600,
        second / 60 % 60,
        second % 60
    );
    if taken != 0 {
        name.push_str(&format!("-{taken}"));
    }
    name
}

/// The civil date (year, month, day) of `days` since 1970-01-01, in the
/// proleptic Gregorian calendar (Howard Hinnant's `civil_from_days`).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests;
