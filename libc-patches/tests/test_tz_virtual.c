/*
 * TDD test for US-207: Patch timezone loading to use virtual filesystem.
 *
 * WARPGRID_SHIM_REQUIRED
 *
 * Tests that localtime()/strftime() correctly use timezone data served
 * by the WarpGrid virtual filesystem shim. Covers:
 *   - POSIX TZ string parsing (UTC, EST5EDT with DST)
 *   - TZif v2 file loading via the virtual filesystem
 *   - DST transitions via POSIX footer in TZif files
 *
 * Compile:
 *   clang --target=wasm32-wasip2 --sysroot=<patched-sysroot> \
 *     -o test_tz_virtual.wasm test_tz_virtual.c
 *
 * Run:
 *   wasmtime run test_tz_virtual.wasm
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* ── Embedded TZif v2 data ─────────────────────────────────────────────── */

/*
 * Minimal TZif v2 for UTC: 0 transitions, 1 type (UTC offset=0),
 * POSIX footer "UTC0".
 */
static const unsigned char TZIF_UTC[] = {
    /* --- V1 header (44 bytes) --- */
    'T','Z','i','f','2',                                    /* magic + version */
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,                         /* reserved (15) */
    0,0,0,0, 0,0,0,0, 0,0,0,0,                             /* isutcnt=0, isstdcnt=0, leapcnt=0 */
    0,0,0,0,                                                /* timecnt=0 */
    0,0,0,1,                                                /* typecnt=1 */
    0,0,0,4,                                                /* charcnt=4 */
    /* --- V1 data (10 bytes) --- */
    0,0,0,0, 0, 0,                                          /* ttinfo: offset=0, isdst=0, idx=0 */
    'U','T','C',0,                                          /* abbrev: "UTC\0" */
    /* --- V2 header (44 bytes) --- */
    'T','Z','i','f','2',
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0,0,0,0, 0,0,0,0, 0,0,0,0,
    0,0,0,0,
    0,0,0,1,
    0,0,0,4,
    /* --- V2 data (10 bytes) --- */
    0,0,0,0, 0, 0,
    'U','T','C',0,
    /* --- Footer (6 bytes) --- */
    '\n','U','T','C','0','\n'
};

/*
 * Minimal TZif v2 for America/New_York (EST5EDT):
 * 1 transition (at INT32_MIN/INT64_MIN) so that scan_trans returns -1
 * for all modern times, falling through to the POSIX footer rules.
 * POSIX footer: "EST5EDT,M3.2.0,M11.1.0"
 */
static const unsigned char TZIF_NY[] = {
    /* --- V1 header (44 bytes) --- */
    'T','Z','i','f','2',
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0,0,0,0, 0,0,0,0, 0,0,0,0,                             /* isutcnt=0, isstdcnt=0, leapcnt=0 */
    0,0,0,1,                                                /* timecnt=1 */
    0,0,0,2,                                                /* typecnt=2 */
    0,0,0,8,                                                /* charcnt=8 */
    /* --- V1 data (25 bytes) --- */
    0x80,0x00,0x00,0x00,                                    /* trans[0] = INT32_MIN */
    0x01,                                                   /* index[0] → type 1 (EDT) */
    0xFF,0xFF,0xB9,0xB0, 0x00, 0x00,                        /* ttinfo[0] EST: offset=-18000, isdst=0, idx=0 */
    0xFF,0xFF,0xC7,0xC0, 0x01, 0x04,                        /* ttinfo[1] EDT: offset=-14400, isdst=1, idx=4 */
    'E','S','T',0, 'E','D','T',0,                           /* abbrev: "EST\0EDT\0" */
    /* --- V2 header (44 bytes) --- */
    'T','Z','i','f','2',
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0,0,0,0, 0,0,0,0, 0,0,0,0,
    0,0,0,1,                                                /* timecnt=1 */
    0,0,0,2,                                                /* typecnt=2 */
    0,0,0,8,                                                /* charcnt=8 */
    /* --- V2 data (29 bytes) --- */
    0x80,0x00,0x00,0x00,0x00,0x00,0x00,0x00,                /* trans[0] = INT64_MIN */
    0x01,                                                   /* index[0] → type 1 */
    0xFF,0xFF,0xB9,0xB0, 0x00, 0x00,                        /* EST */
    0xFF,0xFF,0xC7,0xC0, 0x01, 0x04,                        /* EDT */
    'E','S','T',0, 'E','D','T',0,
    /* --- Footer (24 bytes) --- */
    '\n','E','S','T','5','E','D','T',',',
    'M','3','.','2','.','0',',',
    'M','1','1','.','1','.','0','\n'
};

/* ── Strong override of the WarpGrid filesystem shim ────────────────────── */

int __warpgrid_fs_read_virtual(const char *path,
                               unsigned char *out, int out_len) {
    const unsigned char *data = NULL;
    int data_len = 0;

    if (strcmp(path, "/usr/share/zoneinfo/UTC") == 0 ||
        strcmp(path, "/usr/share/zoneinfo/Etc/UTC") == 0) {
        data = TZIF_UTC;
        data_len = (int)sizeof(TZIF_UTC);
    } else if (strcmp(path, "/usr/share/zoneinfo/America/New_York") == 0 ||
               strcmp(path, "/usr/share/zoneinfo/US/Eastern") == 0) {
        data = TZIF_NY;
        data_len = (int)sizeof(TZIF_NY);
    } else {
        return 0; /* Not a virtual path we handle */
    }

    if (data_len > out_len)
        data_len = out_len;
    memcpy(out, data, data_len);
    return data_len;
}

/* ── Test helpers ───────────────────────────────────────────────────────── */

static int tests_run = 0;
static int tests_passed = 0;

#define TEST(name)                                                      \
    do {                                                                \
        tests_run++;                                                    \
        printf("  TEST: %s ... ", name);                                \
    } while (0)

#define PASS()                                                          \
    do {                                                                \
        tests_passed++;                                                 \
        printf("PASS\n");                                               \
    } while (0)

#define FAIL(msg)                                                       \
    do {                                                                \
        printf("FAIL: %s\n", msg);                                      \
    } while (0)

#define ASSERT(cond, msg)                                               \
    do {                                                                \
        if (!(cond)) { FAIL(msg); return; }                             \
    } while (0)

#define ASSERT_EQ_INT(actual, expected, msg)                            \
    do {                                                                \
        int _a = (actual), _e = (expected);                             \
        if (_a != _e) {                                                 \
            printf("FAIL: %s (got %d, expected %d)\n", msg, _a, _e);    \
            return;                                                     \
        }                                                               \
    } while (0)

/* ── Tests ──────────────────────────────────────────────────────────────── */

/*
 * Test 1: POSIX TZ string "UTC" → localtime returns UTC.
 */
static void test_posix_utc(void) {
    TEST("TZ=UTC → localtime returns UTC offset 0");

    setenv("TZ", "UTC", 1);
    /* localtime_r calls do_tzset() internally */

    /* 2023-11-14 22:13:20 UTC (epoch 1700000000) */
    time_t t = 1700000000;
    struct tm tm;
    struct tm *r = localtime_r(&t, &tm);
    ASSERT(r != NULL, "localtime_r returned NULL");

    ASSERT_EQ_INT(tm.tm_year + 1900, 2023, "year");
    ASSERT_EQ_INT(tm.tm_mon + 1, 11, "month");
    ASSERT_EQ_INT(tm.tm_mday, 14, "day");
    ASSERT_EQ_INT(tm.tm_hour, 22, "hour");
    ASSERT_EQ_INT(tm.tm_min, 13, "min");
    ASSERT_EQ_INT(tm.tm_sec, 20, "sec");
    ASSERT_EQ_INT(tm.tm_isdst, 0, "isdst");

    PASS();
}

/*
 * Test 2: POSIX TZ string "EST5EDT,M3.2.0,M11.1.0" → EST in winter.
 */
static void test_posix_est_winter(void) {
    TEST("TZ=EST5EDT → winter (Nov) gives EST, hour-5");

    setenv("TZ", "EST5EDT,M3.2.0,M11.1.0", 1);
    /* localtime_r calls do_tzset() internally */

    /* 2023-11-14 22:13:20 UTC → 17:13:20 EST (UTC-5) */
    time_t t = 1700000000;
    struct tm tm;
    localtime_r(&t, &tm);

    ASSERT_EQ_INT(tm.tm_hour, 17, "hour should be 17 (UTC-5)");
    ASSERT_EQ_INT(tm.tm_isdst, 0, "isdst should be 0 in winter");

    PASS();
}

/*
 * Test 3: POSIX TZ string "EST5EDT,M3.2.0,M11.1.0" → EDT in summer.
 */
static void test_posix_edt_summer(void) {
    TEST("TZ=EST5EDT → summer (Jul) gives EDT, hour-4");

    setenv("TZ", "EST5EDT,M3.2.0,M11.1.0", 1);
    /* localtime_r calls do_tzset() internally */

    /* 2023-07-22 12:00:00 UTC → 08:00:00 EDT (UTC-4) */
    time_t t = 1690027200;
    struct tm tm;
    localtime_r(&t, &tm);

    ASSERT_EQ_INT(tm.tm_hour, 8, "hour should be 8 (UTC-4)");
    ASSERT_EQ_INT(tm.tm_isdst, 1, "isdst should be 1 in summer");

    PASS();
}

/*
 * Test 4: strftime %Z shows timezone abbreviation.
 */
static void test_strftime_zone_name(void) {
    TEST("strftime %%Z shows timezone abbreviation");

    setenv("TZ", "EST5EDT,M3.2.0,M11.1.0", 1);
    /* localtime_r calls do_tzset() internally */

    /* Winter → EST */
    time_t t = 1700000000;
    struct tm tm;
    localtime_r(&t, &tm);

    char buf[64];
    size_t n = strftime(buf, sizeof(buf), "%Z", &tm);
    ASSERT(n > 0, "strftime returned 0");
    ASSERT(strcmp(buf, "EST") == 0, "expected EST for winter");

    /* Summer → EDT */
    t = 1690027200;
    localtime_r(&t, &tm);
    n = strftime(buf, sizeof(buf), "%Z", &tm);
    ASSERT(n > 0, "strftime returned 0 for summer");
    ASSERT(strcmp(buf, "EDT") == 0, "expected EDT for summer");

    PASS();
}

/*
 * Test 5: TZif file loading via virtual filesystem — UTC.
 * Uses ":UTC" or bare "UTC" name to trigger file search at
 * /usr/share/zoneinfo/UTC.
 *
 * Note: "UTC" alone is detected as a POSIX string (it matches the
 * "UTC"/"GMT" special case in do_tzset). We use ":UTC" to force
 * file-based loading (the ':' prefix means "this is a file path").
 */
static void test_tzif_file_utc(void) {
    TEST("TZ=:UTC → loads TZif file from virtual FS");

    setenv("TZ", ":UTC", 1);
    /* localtime_r calls do_tzset() internally */

    time_t t = 1700000000;
    struct tm tm;
    localtime_r(&t, &tm);

    ASSERT_EQ_INT(tm.tm_hour, 22, "hour should be 22 (UTC)");
    ASSERT_EQ_INT(tm.tm_isdst, 0, "isdst should be 0");

    char buf[64];
    strftime(buf, sizeof(buf), "%Z", &tm);
    ASSERT(strcmp(buf, "UTC") == 0, "zone name should be UTC");

    PASS();
}

/*
 * Test 6: TZif file loading via search path — America/New_York.
 * "America/New_York" is not a POSIX string, so do_tzset searches
 * /usr/share/zoneinfo/America/New_York, which our shim serves.
 */
static void test_tzif_file_new_york_winter(void) {
    TEST("TZ=America/New_York → loads TZif, winter gives EST");

    setenv("TZ", "America/New_York", 1);
    /* localtime_r calls do_tzset() internally */

    /* 2023-11-14 22:13:20 UTC → 17:13:20 EST */
    time_t t = 1700000000;
    struct tm tm;
    localtime_r(&t, &tm);

    ASSERT_EQ_INT(tm.tm_hour, 17, "hour should be 17 (EST)");
    ASSERT_EQ_INT(tm.tm_isdst, 0, "isdst should be 0 in winter");

    char buf[64];
    strftime(buf, sizeof(buf), "%Z", &tm);
    ASSERT(strcmp(buf, "EST") == 0, "zone name should be EST");

    PASS();
}

/*
 * Test 7: TZif file loading — America/New_York summer → EDT.
 */
static void test_tzif_file_new_york_summer(void) {
    TEST("TZ=America/New_York → loads TZif, summer gives EDT");

    setenv("TZ", "America/New_York", 1);
    /* localtime_r calls do_tzset() internally */

    /* 2023-07-22 12:00:00 UTC → 08:00:00 EDT */
    time_t t = 1690027200;
    struct tm tm;
    localtime_r(&t, &tm);

    ASSERT_EQ_INT(tm.tm_hour, 8, "hour should be 8 (EDT)");
    ASSERT_EQ_INT(tm.tm_isdst, 1, "isdst should be 1 in summer");

    char buf[64];
    strftime(buf, sizeof(buf), "%Z", &tm);
    ASSERT(strcmp(buf, "EDT") == 0, "zone name should be EDT");

    PASS();
}

/*
 * Test 8: mktime round-trips correctly with timezone offset.
 */
static void test_mktime_roundtrip(void) {
    TEST("mktime round-trips with EST5EDT");

    setenv("TZ", "EST5EDT,M3.2.0,M11.1.0", 1);
    /* localtime_r calls do_tzset() internally */

    /* Start with a known UTC time */
    time_t original = 1700000000;
    struct tm tm;
    localtime_r(&original, &tm);

    /* mktime should give back the same epoch value */
    time_t roundtrip = mktime(&tm);
    ASSERT(roundtrip == original, "mktime should round-trip to original epoch");

    PASS();
}

/*
 * Test 9: Switching TZ between calls works correctly.
 */
static void test_tz_switch(void) {
    TEST("switching TZ between localtime calls");

    time_t t = 1700000000;
    struct tm tm;

    /* First: UTC */
    setenv("TZ", "UTC", 1);
    /* localtime_r calls do_tzset() internally */
    localtime_r(&t, &tm);
    ASSERT_EQ_INT(tm.tm_hour, 22, "UTC hour should be 22");

    /* Switch to EST */
    setenv("TZ", "EST5EDT,M3.2.0,M11.1.0", 1);
    /* localtime_r calls do_tzset() internally */
    localtime_r(&t, &tm);
    ASSERT_EQ_INT(tm.tm_hour, 17, "EST hour should be 17");

    /* Switch back to UTC */
    setenv("TZ", "UTC", 1);
    /* localtime_r calls do_tzset() internally */
    localtime_r(&t, &tm);
    ASSERT_EQ_INT(tm.tm_hour, 22, "UTC hour should be 22 again");

    PASS();
}

/*
 * Test 10: Unknown timezone falls back to UTC without crashing.
 * AC #3: "Unknown timezone falls back to UTC without crashing."
 */
static void test_unknown_timezone_fallback(void) {
    TEST("TZ=Mars/Olympus → falls back to UTC without crashing");

    setenv("TZ", "Mars/Olympus", 1);

    time_t t = 1700000000;
    struct tm tm;
    struct tm *r = localtime_r(&t, &tm);
    ASSERT(r != NULL, "localtime_r returned NULL for unknown timezone");

    /* With no TZif file found and no valid POSIX string, musl falls back
     * to UTC (s = __utc). Verify UTC offset behavior. */
    ASSERT_EQ_INT(tm.tm_hour, 22, "hour should be 22 (UTC fallback)");
    ASSERT_EQ_INT(tm.tm_isdst, 0, "isdst should be 0 (UTC fallback)");

    PASS();
}

/*
 * Test 11: Europe/London strftime %Z shows GMT in winter and BST in summer.
 * AC #2: "strftime %Z outputs correct timezone abbreviation for Europe/London."
 * Note: This test uses POSIX TZ string since our shim doesn't embed London
 * TZif data. The POSIX string exercises the same do_tzset() code path.
 */
static void test_london_strftime_zone_name(void) {
    TEST("TZ=GMT0BST → strftime %%Z shows GMT/BST");

    setenv("TZ", "GMT0BST,M3.5.0/1,M10.5.0", 1);

    /* Winter (November) → GMT */
    time_t t = 1700000000; /* 2023-11-14 22:13:20 UTC */
    struct tm tm;
    localtime_r(&t, &tm);

    char buf[64];
    size_t n = strftime(buf, sizeof(buf), "%Z", &tm);
    ASSERT(n > 0, "strftime returned 0 for winter");
    ASSERT(strcmp(buf, "GMT") == 0, "expected GMT in winter");
    ASSERT_EQ_INT(tm.tm_hour, 22, "hour should be 22 (GMT, same as UTC)");

    /* Summer (July) → BST (+1) */
    t = 1690027200; /* 2023-07-22 12:00:00 UTC → 13:00 BST */
    localtime_r(&t, &tm);
    n = strftime(buf, sizeof(buf), "%Z", &tm);
    ASSERT(n > 0, "strftime returned 0 for summer");
    ASSERT(strcmp(buf, "BST") == 0, "expected BST in summer");
    ASSERT_EQ_INT(tm.tm_hour, 13, "hour should be 13 (BST, UTC+1)");

    PASS();
}

/* ── Main ───────────────────────────────────────────────────────────────── */

int main(void) {
    printf("=== US-207: Timezone loading via virtual filesystem ===\n\n");

    test_posix_utc();
    test_posix_est_winter();
    test_posix_edt_summer();
    test_strftime_zone_name();
    test_tzif_file_utc();
    test_tzif_file_new_york_winter();
    test_tzif_file_new_york_summer();
    test_mktime_roundtrip();
    test_tz_switch();
    test_unknown_timezone_fallback();
    test_london_strftime_zone_name();

    printf("\n=== Results: %d/%d passed ===\n", tests_passed, tests_run);
    return (tests_passed == tests_run) ? 0 : 1;
}
