//! TZif v2 binary data generation for WarpGrid virtual filesystem.
//!
//! Generates spec-compliant TZif v2 files (RFC 8536) that musl's `__tz.c` parser
//! can read to determine UTC offsets, DST transitions, and timezone abbreviations.
//!
//! Each timezone is defined by a [`TzSpec`] with standard time info, optional DST
//! rules, and a POSIX TZ string. For DST zones, concrete transition timestamps are
//! generated for years 2020–2038 in the v2 block. The POSIX TZ string footer
//! handles dates outside this range.

use std::collections::HashMap;

// ── Date math helpers ───────────────────────────────────────────────────

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(month: u32, year: i32) -> u32 {
    match month {
        1 => 31,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        3 => 31,
        4 => 30,
        5 => 31,
        6 => 30,
        7 => 31,
        8 => 31,
        9 => 30,
        10 => 31,
        11 => 30,
        12 => 31,
        _ => panic!("invalid month: {month}"),
    }
}

/// Days from 1970-01-01 to the given date (Howard Hinnant's algorithm).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 {
        year as i64 - 1
    } else {
        year as i64
    };
    let m = if month <= 2 {
        month as i64 + 9
    } else {
        month as i64 - 3
    };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = (y - era * 400) as u64;
    let doy = (153 * m as u64 + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

/// Day of week for a date (0=Sunday, 1=Monday, …, 6=Saturday).
fn weekday(year: i32, month: u32, day: u32) -> u32 {
    // 1970-01-01 was Thursday (4).
    let days = days_from_civil(year, month, day);
    ((days % 7 + 4 + 7) % 7) as u32
}

/// Find the Nth occurrence of a weekday in a month.
///
/// `week`: 1–4 for specific, 5 for "last".
/// `weekday_target`: 0=Sunday.
fn nth_weekday_in_month(year: i32, month: u32, week: u32, weekday_target: u32) -> u32 {
    if week == 5 {
        // Last occurrence: work backward from end of month.
        let last_day = days_in_month(month, year);
        let wd = weekday(year, month, last_day);
        let diff = (wd + 7 - weekday_target) % 7;
        last_day - diff
    } else {
        // Nth occurrence: find first, then skip (N-1) weeks.
        let wd = weekday(year, month, 1);
        let first = 1 + (weekday_target + 7 - wd) % 7;
        first + (week - 1) * 7
    }
}

// ── TZif types ──────────────────────────────────────────────────────────

/// DST transition rule in POSIX M.w.d format.
pub struct DstRule {
    /// Month (1–12).
    pub month: u32,
    /// Week (1–4 for specific, 5 for "last").
    pub week: u32,
    /// Day of week (0=Sunday).
    pub weekday: u32,
    /// Time of transition in seconds from midnight UTC.
    pub utc_time: i32,
}

/// DST specification for a timezone.
pub struct DstSpec {
    /// DST abbreviation (e.g., "EDT").
    pub abbrev: &'static str,
    /// DST UTC offset in seconds (e.g., -14400 for EDT).
    pub offset: i32,
    /// When DST starts (spring forward).
    pub start: DstRule,
    /// When DST ends (fall back).
    pub end: DstRule,
}

/// Timezone specification for TZif generation.
pub struct TzSpec {
    /// Standard time abbreviation (e.g., "EST").
    pub std_abbrev: &'static str,
    /// Standard time UTC offset in seconds (negative for west).
    pub std_offset: i32,
    /// DST specification, if the zone observes DST.
    pub dst: Option<DstSpec>,
    /// POSIX TZ string for the footer (e.g., "EST5EDT,M3.2.0,M11.1.0").
    pub posix_tz: &'static str,
}

// ── Binary generation ───────────────────────────────────────────────────

/// Range of years for concrete transition timestamps in the v2 block.
const TRANSITION_YEAR_START: i32 = 2020;
const TRANSITION_YEAR_END: i32 = 2038;

/// Generate transition timestamps for a DST zone over the configured year range.
///
/// Returns pairs of (unix_timestamp, type_index) sorted by timestamp.
/// Type 0 = standard, type 1 = DST.
fn generate_transitions(dst: &DstSpec) -> Vec<(i64, u8)> {
    let mut transitions = Vec::new();

    for year in TRANSITION_YEAR_START..=TRANSITION_YEAR_END {
        // Spring forward: STD → DST (type 1)
        let start_day =
            nth_weekday_in_month(year, dst.start.month, dst.start.week, dst.start.weekday);
        let start_ts =
            days_from_civil(year, dst.start.month, start_day) * 86400 + dst.start.utc_time as i64;
        transitions.push((start_ts, 1));

        // Fall back: DST → STD (type 0)
        let end_day = nth_weekday_in_month(year, dst.end.month, dst.end.week, dst.end.weekday);
        let end_ts =
            days_from_civil(year, dst.end.month, end_day) * 86400 + dst.end.utc_time as i64;
        transitions.push((end_ts, 0));
    }

    transitions.sort_by_key(|&(ts, _)| ts);
    transitions
}

/// Write a 44-byte TZif header.
fn write_header(
    out: &mut Vec<u8>,
    version: u8,
    ttisutcnt: u32,
    ttisstdcnt: u32,
    leapcnt: u32,
    timecnt: u32,
    typecnt: u32,
    charcnt: u32,
) {
    out.extend_from_slice(b"TZif");
    out.push(version);
    out.extend_from_slice(&[0u8; 15]); // reserved
    out.extend_from_slice(&ttisutcnt.to_be_bytes());
    out.extend_from_slice(&ttisstdcnt.to_be_bytes());
    out.extend_from_slice(&leapcnt.to_be_bytes());
    out.extend_from_slice(&timecnt.to_be_bytes());
    out.extend_from_slice(&typecnt.to_be_bytes());
    out.extend_from_slice(&charcnt.to_be_bytes());
}

/// Write a 6-byte ttinfo record.
fn write_ttinfo(out: &mut Vec<u8>, utoff: i32, isdst: u8, desigidx: u8) {
    out.extend_from_slice(&utoff.to_be_bytes());
    out.push(isdst);
    out.push(desigidx);
}

/// Generate a valid TZif v2 binary blob for the given timezone specification.
///
/// The output structure:
/// 1. v1 block — minimal UTC fallback (0 transitions, 1 type)
/// 2. v2 block — real data with 64-bit transition timestamps
/// 3. POSIX TZ string footer — drives DST rules for all dates
///
/// musl's `__tz.c` uses the POSIX footer for times beyond the last transition,
/// so the transition table only needs to cover a practical range (2020–2038).
pub fn generate_tzif(spec: &TzSpec) -> Vec<u8> {
    let mut out = Vec::with_capacity(512);

    // ── v1 block (minimal UTC fallback) ──
    let v1_abbrev = b"UTC\0";
    write_header(&mut out, b'2', 0, 0, 0, 0, 1, v1_abbrev.len() as u32);
    write_ttinfo(&mut out, 0, 0, 0);
    out.extend_from_slice(v1_abbrev);

    // ── v2 block ──

    // Build abbreviation string pool.
    let mut v2_abbrev = Vec::new();
    v2_abbrev.extend_from_slice(spec.std_abbrev.as_bytes());
    v2_abbrev.push(0);
    let dst_desigidx = v2_abbrev.len() as u8;
    if let Some(ref dst) = spec.dst {
        v2_abbrev.extend_from_slice(dst.abbrev.as_bytes());
        v2_abbrev.push(0);
    }

    let v2_typecnt: u32 = if spec.dst.is_some() { 2 } else { 1 };

    // Generate transitions.
    let transitions = match &spec.dst {
        Some(dst) => generate_transitions(dst),
        None => Vec::new(),
    };
    let v2_timecnt = transitions.len() as u32;

    write_header(
        &mut out,
        b'2',
        0,
        0,
        0,
        v2_timecnt,
        v2_typecnt,
        v2_abbrev.len() as u32,
    );

    // Transition times (64-bit big-endian).
    for &(ts, _) in &transitions {
        out.extend_from_slice(&ts.to_be_bytes());
    }

    // Transition type indices (1 byte each).
    for &(_, idx) in &transitions {
        out.push(idx);
    }

    // ttinfo records.
    write_ttinfo(&mut out, spec.std_offset, 0, 0); // type 0: standard
    if let Some(ref dst) = spec.dst {
        write_ttinfo(&mut out, dst.offset, 1, dst_desigidx); // type 1: DST
    }

    // Abbreviation pool.
    out.extend_from_slice(&v2_abbrev);

    // No leap seconds, std/wall indicators, or UT/local indicators.

    // ── POSIX TZ string footer ──
    out.push(b'\n');
    out.extend_from_slice(spec.posix_tz.as_bytes());
    out.push(b'\n');

    out
}

/// US DST rules: 2nd Sunday March → 1st Sunday November.
fn us_dst(
    std_abbrev: &'static str,
    std_offset: i32,
    dst_abbrev: &'static str,
    dst_offset: i32,
    posix_tz: &'static str,
) -> TzSpec {
    // Spring forward: 2:00 AM local standard → UTC = local + |offset|
    let spring_utc = 2 * 3600 + std_offset.unsigned_abs() as i32;
    // Fall back: 2:00 AM local DST → UTC = local + |dst_offset|
    let fall_utc = 2 * 3600 + dst_offset.unsigned_abs() as i32;

    TzSpec {
        std_abbrev,
        std_offset,
        dst: Some(DstSpec {
            abbrev: dst_abbrev,
            offset: dst_offset,
            start: DstRule {
                month: 3,
                week: 2,
                weekday: 0,
                utc_time: spring_utc,
            },
            end: DstRule {
                month: 11,
                week: 1,
                weekday: 0,
                utc_time: fall_utc,
            },
        }),
        posix_tz,
    }
}

/// Return a map of timezone name → TZif v2 binary data for all default timezones.
///
/// Covers at minimum: UTC, America/New_York, US/Eastern, America/Chicago,
/// America/Denver, America/Los_Angeles, US/Pacific, Europe/London, Asia/Tokyo.
pub fn default_timezone_data() -> HashMap<String, Vec<u8>> {
    let specs: Vec<(&str, TzSpec)> = vec![
        (
            "UTC",
            TzSpec {
                std_abbrev: "UTC",
                std_offset: 0,
                dst: None,
                posix_tz: "UTC0",
            },
        ),
        (
            "America/New_York",
            us_dst("EST", -18000, "EDT", -14400, "EST5EDT,M3.2.0,M11.1.0"),
        ),
        (
            "US/Eastern",
            us_dst("EST", -18000, "EDT", -14400, "EST5EDT,M3.2.0,M11.1.0"),
        ),
        (
            "America/Chicago",
            us_dst("CST", -21600, "CDT", -18000, "CST6CDT,M3.2.0,M11.1.0"),
        ),
        (
            "America/Denver",
            us_dst("MST", -25200, "MDT", -21600, "MST7MDT,M3.2.0,M11.1.0"),
        ),
        (
            "America/Los_Angeles",
            us_dst("PST", -28800, "PDT", -25200, "PST8PDT,M3.2.0,M11.1.0"),
        ),
        (
            "US/Pacific",
            us_dst("PST", -28800, "PDT", -25200, "PST8PDT,M3.2.0,M11.1.0"),
        ),
        (
            "Europe/London",
            TzSpec {
                std_abbrev: "GMT",
                std_offset: 0,
                dst: Some(DstSpec {
                    abbrev: "BST",
                    offset: 3600,
                    start: DstRule {
                        month: 3,
                        week: 5,
                        weekday: 0,
                        utc_time: 3600, // 01:00 UTC
                    },
                    end: DstRule {
                        month: 10,
                        week: 5,
                        weekday: 0,
                        utc_time: 3600, // 01:00 UTC
                    },
                }),
                posix_tz: "GMT0BST,M3.5.0/1,M10.5.0",
            },
        ),
        (
            "Asia/Tokyo",
            TzSpec {
                std_abbrev: "JST",
                std_offset: 32400, // +9h
                dst: None,
                posix_tz: "JST-9",
            },
        ),
    ];

    specs
        .into_iter()
        .map(|(name, spec)| (name.to_string(), generate_tzif(&spec)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── TZif binary parser helpers ──────────────────────────────────────

    fn read_be_u32(data: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    }

    fn read_be_i32(data: &[u8], offset: usize) -> i32 {
        i32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    }

    fn read_be_i64(data: &[u8], offset: usize) -> i64 {
        i64::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ])
    }

    // ── Date math ───────────────────────────────────────────────────────

    #[test]
    fn days_from_civil_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn days_from_civil_known_date() {
        // 2024-03-10 = 19792 days since epoch
        assert_eq!(days_from_civil(2024, 3, 10), 19792);
    }

    #[test]
    fn weekday_known_dates() {
        // 1970-01-01 = Thursday (4)
        assert_eq!(weekday(1970, 1, 1), 4);
        // 2024-03-10 = Sunday (0)
        assert_eq!(weekday(2024, 3, 10), 0);
        // 2024-03-31 = Sunday (0)
        assert_eq!(weekday(2024, 3, 31), 0);
    }

    #[test]
    fn nth_weekday_second_sunday_march_2024() {
        // 2nd Sunday in March 2024 = March 10
        assert_eq!(nth_weekday_in_month(2024, 3, 2, 0), 10);
    }

    #[test]
    fn nth_weekday_first_sunday_november_2024() {
        // 1st Sunday in November 2024 = November 3
        assert_eq!(nth_weekday_in_month(2024, 11, 1, 0), 3);
    }

    #[test]
    fn nth_weekday_last_sunday_march_2024() {
        // Last Sunday in March 2024 = March 31
        assert_eq!(nth_weekday_in_month(2024, 3, 5, 0), 31);
    }

    #[test]
    fn nth_weekday_last_sunday_march_2025() {
        // March 31, 2025 = Monday → last Sunday = March 30
        assert_eq!(nth_weekday_in_month(2025, 3, 5, 0), 30);
    }

    #[test]
    fn nth_weekday_last_sunday_october_2024() {
        // Last Sunday in October 2024 = October 27
        assert_eq!(nth_weekday_in_month(2024, 10, 5, 0), 27);
    }

    // ── TZif v2 structure validation ────────────────────────────────────

    /// Parse v1 header and return (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt).
    fn parse_v1_header(data: &[u8]) -> (u32, u32, u32, u32, u32, u32) {
        assert!(data.len() >= 44, "data too short for v1 header");
        assert_eq!(&data[0..4], b"TZif", "missing TZif magic");
        assert_eq!(data[4], b'2', "expected version '2'");
        (
            read_be_u32(data, 20),
            read_be_u32(data, 24),
            read_be_u32(data, 28),
            read_be_u32(data, 32),
            read_be_u32(data, 36),
            read_be_u32(data, 40),
        )
    }

    /// Compute v1 data block size from header counts.
    fn v1_data_size(
        ttisutcnt: u32,
        ttisstdcnt: u32,
        leapcnt: u32,
        timecnt: u32,
        typecnt: u32,
        charcnt: u32,
    ) -> usize {
        (timecnt * 5 + typecnt * 6 + charcnt + leapcnt * 8 + ttisstdcnt + ttisutcnt) as usize
    }

    /// Parse v2 header starting at a given offset, return header counts and offset past header.
    fn parse_v2_header(
        data: &[u8],
        offset: usize,
    ) -> ((u32, u32, u32, u32, u32, u32), usize) {
        assert!(
            data.len() >= offset + 44,
            "data too short for v2 header at offset {offset}"
        );
        assert_eq!(
            &data[offset..offset + 4],
            b"TZif",
            "missing TZif magic in v2 header"
        );
        let counts = (
            read_be_u32(data, offset + 20),
            read_be_u32(data, offset + 24),
            read_be_u32(data, offset + 28),
            read_be_u32(data, offset + 32),
            read_be_u32(data, offset + 36),
            read_be_u32(data, offset + 40),
        );
        (counts, offset + 44)
    }

    /// Extract the POSIX TZ string footer from the end of TZif data.
    fn extract_posix_footer(data: &[u8]) -> &str {
        assert_eq!(data[data.len() - 1], b'\n', "footer must end with newline");
        // Scan backward for the second-to-last newline.
        let end = data.len() - 1;
        let mut start = end - 1;
        while data[start] != b'\n' {
            start -= 1;
        }
        std::str::from_utf8(&data[start + 1..end]).expect("footer must be valid UTF-8")
    }

    // ── UTC ─────────────────────────────────────────────────────────────

    #[test]
    fn utc_tzif_magic_and_version() {
        let data = generate_tzif(&TzSpec {
            std_abbrev: "UTC",
            std_offset: 0,
            dst: None,
            posix_tz: "UTC0",
        });
        assert!(data.starts_with(b"TZif"));
        assert_eq!(data[4], b'2');
    }

    #[test]
    fn utc_tzif_v1_header() {
        let data = generate_tzif(&TzSpec {
            std_abbrev: "UTC",
            std_offset: 0,
            dst: None,
            posix_tz: "UTC0",
        });
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        assert_eq!(timecnt, 0);
        assert_eq!(typecnt, 1);
        assert_eq!(charcnt, 4); // "UTC\0"
        assert_eq!(leapcnt, 0);
        assert_eq!(ttisutcnt, 0);
        assert_eq!(ttisstdcnt, 0);
    }

    #[test]
    fn utc_tzif_v2_has_correct_type() {
        let data = generate_tzif(&TzSpec {
            std_abbrev: "UTC",
            std_offset: 0,
            dst: None,
            posix_tz: "UTC0",
        });
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        let v1_skip = v1_data_size(ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt);
        let v2_start = 44 + v1_skip;

        let ((_, _, _, v2_timecnt, v2_typecnt, v2_charcnt), v2_data) =
            parse_v2_header(&data, v2_start);
        assert_eq!(v2_timecnt, 0);
        assert_eq!(v2_typecnt, 1);

        // ttinfo[0]: offset=0, isdst=0, desigidx=0
        let utoff = read_be_i32(&data, v2_data);
        assert_eq!(utoff, 0, "UTC offset should be 0");
        assert_eq!(data[v2_data + 4], 0, "isdst should be 0");
        assert_eq!(data[v2_data + 5], 0, "desigidx should be 0");

        // Abbreviation pool starts after ttinfo records.
        let abbrev_start = v2_data + (v2_typecnt as usize) * 6;
        let abbrev = &data[abbrev_start..abbrev_start + v2_charcnt as usize];
        assert_eq!(abbrev, b"UTC\0");
    }

    #[test]
    fn utc_tzif_footer() {
        let data = generate_tzif(&TzSpec {
            std_abbrev: "UTC",
            std_offset: 0,
            dst: None,
            posix_tz: "UTC0",
        });
        assert_eq!(extract_posix_footer(&data), "UTC0");
    }

    #[test]
    fn utc_tzif_size_within_limits() {
        let data = generate_tzif(&TzSpec {
            std_abbrev: "UTC",
            std_offset: 0,
            dst: None,
            posix_tz: "UTC0",
        });
        assert!(data.len() < 8192, "TZif data exceeds 8KiB limit");
    }

    // ── America/New_York ────────────────────────────────────────────────

    fn new_york_spec() -> TzSpec {
        us_dst("EST", -18000, "EDT", -14400, "EST5EDT,M3.2.0,M11.1.0")
    }

    #[test]
    fn new_york_tzif_has_transitions() {
        let data = generate_tzif(&new_york_spec());
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        let v1_skip = v1_data_size(ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt);
        let v2_start = 44 + v1_skip;

        let ((_, _, _, v2_timecnt, v2_typecnt, _), _) = parse_v2_header(&data, v2_start);

        // 2020–2038 = 19 years × 2 transitions = 38
        assert_eq!(v2_timecnt, 38);
        assert_eq!(v2_typecnt, 2);
    }

    #[test]
    fn new_york_tzif_types() {
        let data = generate_tzif(&new_york_spec());
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        let v1_skip = v1_data_size(ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt);
        let v2_start = 44 + v1_skip;

        let ((_, _, _, v2_timecnt, _, v2_charcnt), v2_data) = parse_v2_header(&data, v2_start);

        // Skip past transition times and indices to reach ttinfo records.
        let ttinfo_start = v2_data + (v2_timecnt as usize) * 8 + (v2_timecnt as usize);

        // type 0: EST (offset=-18000, isdst=0, desigidx=0)
        assert_eq!(read_be_i32(&data, ttinfo_start), -18000);
        assert_eq!(data[ttinfo_start + 4], 0); // isdst=0
        assert_eq!(data[ttinfo_start + 5], 0); // desigidx=0

        // type 1: EDT (offset=-14400, isdst=1, desigidx=4)
        assert_eq!(read_be_i32(&data, ttinfo_start + 6), -14400);
        assert_eq!(data[ttinfo_start + 6 + 4], 1); // isdst=1
        assert_eq!(data[ttinfo_start + 6 + 5], 4); // desigidx=4

        // Abbreviation pool: "EST\0EDT\0"
        let abbrev_start = ttinfo_start + 2 * 6;
        let abbrev = &data[abbrev_start..abbrev_start + v2_charcnt as usize];
        assert_eq!(abbrev, b"EST\0EDT\0");
    }

    #[test]
    fn new_york_tzif_2024_spring_forward_timestamp() {
        let data = generate_tzif(&new_york_spec());
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        let v1_skip = v1_data_size(ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt);
        let v2_start = 44 + v1_skip;
        let ((_, _, _, v2_timecnt, _, _), v2_data) = parse_v2_header(&data, v2_start);

        // 2024 spring forward: March 10, 2024 at 07:00 UTC
        // = days_from_civil(2024, 3, 10) * 86400 + 7*3600
        // = 19792 * 86400 + 25200 = 1710054000
        let expected_ts: i64 = 1710054000;

        // Find this timestamp among v2 transitions.
        let mut found = false;
        for i in 0..v2_timecnt as usize {
            let ts = read_be_i64(&data, v2_data + i * 8);
            if ts == expected_ts {
                // Check the type index for this transition.
                let idx_offset = v2_data + (v2_timecnt as usize) * 8 + i;
                assert_eq!(data[idx_offset], 1, "spring forward should switch to DST (type 1)");
                found = true;
                break;
            }
        }
        assert!(found, "2024 spring forward timestamp not found in transitions");
    }

    #[test]
    fn new_york_tzif_2024_fall_back_timestamp() {
        let data = generate_tzif(&new_york_spec());
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        let v1_skip = v1_data_size(ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt);
        let v2_start = 44 + v1_skip;
        let ((_, _, _, v2_timecnt, _, _), v2_data) = parse_v2_header(&data, v2_start);

        // 2024 fall back: November 3, 2024 at 06:00 UTC
        // = days_from_civil(2024, 11, 3) * 86400 + 6*3600
        // = 20030 * 86400 + 21600 = 1730613600
        let expected_ts: i64 = 1730613600;

        let mut found = false;
        for i in 0..v2_timecnt as usize {
            let ts = read_be_i64(&data, v2_data + i * 8);
            if ts == expected_ts {
                let idx_offset = v2_data + (v2_timecnt as usize) * 8 + i;
                assert_eq!(data[idx_offset], 0, "fall back should switch to STD (type 0)");
                found = true;
                break;
            }
        }
        assert!(found, "2024 fall back timestamp not found in transitions");
    }

    #[test]
    fn new_york_tzif_footer() {
        let data = generate_tzif(&new_york_spec());
        assert_eq!(extract_posix_footer(&data), "EST5EDT,M3.2.0,M11.1.0");
    }

    #[test]
    fn new_york_tzif_contains_abbreviations() {
        let data = generate_tzif(&new_york_spec());
        assert!(data.windows(3).any(|w| w == b"EST"));
        assert!(data.windows(3).any(|w| w == b"EDT"));
    }

    // ── Europe/London ───────────────────────────────────────────────────

    fn london_spec() -> TzSpec {
        TzSpec {
            std_abbrev: "GMT",
            std_offset: 0,
            dst: Some(DstSpec {
                abbrev: "BST",
                offset: 3600,
                start: DstRule {
                    month: 3,
                    week: 5,
                    weekday: 0,
                    utc_time: 3600,
                },
                end: DstRule {
                    month: 10,
                    week: 5,
                    weekday: 0,
                    utc_time: 3600,
                },
            }),
            posix_tz: "GMT0BST,M3.5.0/1,M10.5.0",
        }
    }

    #[test]
    fn london_tzif_types() {
        let data = generate_tzif(&london_spec());
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        let v1_skip = v1_data_size(ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt);
        let v2_start = 44 + v1_skip;
        let ((_, _, _, v2_timecnt, _, v2_charcnt), v2_data) = parse_v2_header(&data, v2_start);

        let ttinfo_start = v2_data + (v2_timecnt as usize) * 8 + (v2_timecnt as usize);

        // type 0: GMT (offset=0, isdst=0)
        assert_eq!(read_be_i32(&data, ttinfo_start), 0);
        assert_eq!(data[ttinfo_start + 4], 0);

        // type 1: BST (offset=3600, isdst=1)
        assert_eq!(read_be_i32(&data, ttinfo_start + 6), 3600);
        assert_eq!(data[ttinfo_start + 6 + 4], 1);

        // Abbreviation pool: "GMT\0BST\0"
        let abbrev_start = ttinfo_start + 2 * 6;
        let abbrev = &data[abbrev_start..abbrev_start + v2_charcnt as usize];
        assert_eq!(abbrev, b"GMT\0BST\0");
    }

    #[test]
    fn london_tzif_footer() {
        let data = generate_tzif(&london_spec());
        assert_eq!(extract_posix_footer(&data), "GMT0BST,M3.5.0/1,M10.5.0");
    }

    // ── Asia/Tokyo ──────────────────────────────────────────────────────

    #[test]
    fn tokyo_tzif_no_transitions() {
        let data = generate_tzif(&TzSpec {
            std_abbrev: "JST",
            std_offset: 32400,
            dst: None,
            posix_tz: "JST-9",
        });
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        let v1_skip = v1_data_size(ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt);
        let v2_start = 44 + v1_skip;
        let ((_, _, _, v2_timecnt, v2_typecnt, _), v2_data) = parse_v2_header(&data, v2_start);

        assert_eq!(v2_timecnt, 0);
        assert_eq!(v2_typecnt, 1);
        assert_eq!(read_be_i32(&data, v2_data), 32400);
    }

    #[test]
    fn tokyo_tzif_footer() {
        let data = generate_tzif(&TzSpec {
            std_abbrev: "JST",
            std_offset: 32400,
            dst: None,
            posix_tz: "JST-9",
        });
        assert_eq!(extract_posix_footer(&data), "JST-9");
    }

    // ── default_timezone_data ───────────────────────────────────────────

    #[test]
    fn default_data_has_all_zones() {
        let zones = default_timezone_data();
        let expected = [
            "UTC",
            "America/New_York",
            "US/Eastern",
            "America/Chicago",
            "America/Denver",
            "America/Los_Angeles",
            "US/Pacific",
            "Europe/London",
            "Asia/Tokyo",
        ];
        for name in &expected {
            assert!(
                zones.contains_key(*name),
                "missing default timezone: {name}"
            );
        }
    }

    #[test]
    fn default_data_all_start_with_tzif_magic() {
        let zones = default_timezone_data();
        for (name, data) in &zones {
            assert!(
                data.starts_with(b"TZif"),
                "{name}: does not start with TZif magic"
            );
        }
    }

    #[test]
    fn default_data_all_within_size_limit() {
        let zones = default_timezone_data();
        for (name, data) in &zones {
            assert!(
                data.len() < 8192,
                "{name}: TZif data ({} bytes) exceeds 8KiB limit",
                data.len()
            );
        }
    }

    #[test]
    fn default_data_us_eastern_matches_new_york() {
        let zones = default_timezone_data();
        assert_eq!(
            zones["US/Eastern"], zones["America/New_York"],
            "US/Eastern should have identical TZif data to America/New_York"
        );
    }

    #[test]
    fn default_data_us_pacific_matches_los_angeles() {
        let zones = default_timezone_data();
        assert_eq!(
            zones["US/Pacific"], zones["America/Los_Angeles"],
            "US/Pacific should have identical TZif data to America/Los_Angeles"
        );
    }

    // ── Transitions are sorted ──────────────────────────────────────────

    #[test]
    fn transitions_are_monotonically_increasing() {
        let data = generate_tzif(&new_york_spec());
        let (ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt) = parse_v1_header(&data);
        let v1_skip = v1_data_size(ttisutcnt, ttisstdcnt, leapcnt, timecnt, typecnt, charcnt);
        let v2_start = 44 + v1_skip;
        let ((_, _, _, v2_timecnt, _, _), v2_data) = parse_v2_header(&data, v2_start);

        let mut prev = i64::MIN;
        for i in 0..v2_timecnt as usize {
            let ts = read_be_i64(&data, v2_data + i * 8);
            assert!(
                ts > prev,
                "transition {i}: {ts} is not greater than previous {prev}"
            );
            prev = ts;
        }
    }
}
