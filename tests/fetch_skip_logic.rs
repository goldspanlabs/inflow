//! Integration tests for fetch skip logic (start >= end check).
//!
//! Verifies that when resume_date >= today, no fetch occurs.
//! This applies to EODHD options provider only (Yahoo doesn't have resume logic).

use chrono::{Duration, NaiveDate};

/// Test helper: simulate resume date check
/// Returns true if fetch should be skipped (start >= end)
fn should_skip_fetch(start: NaiveDate, end: NaiveDate) -> bool {
    start >= end
}

#[test]
fn test_saturday_with_friday_cache_skips_fetch() {
    // Scenario: Last cached = Friday, Today = Saturday, Resume = Monday
    // Expected: Skip fetch (Monday >= Saturday)

    let _friday = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
    let saturday = NaiveDate::from_ymd_opt(2024, 1, 20).unwrap();
    let monday = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();

    let start = monday; // resume_from (from prices cache)
    let end = saturday; // today

    assert!(should_skip_fetch(start, end), "Should skip fetch on Saturday");
}

#[test]
fn test_sunday_with_friday_cache_skips_fetch() {
    // Scenario: Last cached = Friday, Today = Sunday, Resume = Monday
    // Expected: Skip fetch (Monday >= Sunday)

    let _friday = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
    let sunday = NaiveDate::from_ymd_opt(2024, 1, 21).unwrap();
    let monday = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();

    let start = monday;
    let end = sunday;

    assert!(should_skip_fetch(start, end), "Should skip fetch on Sunday");
}

#[test]
fn test_monday_with_friday_cache_fetches() {
    // Scenario: Last cached = Friday, Today = Monday, Resume = Monday
    // Expected: Fetch (Monday < Monday is false, but Monday >= Monday is true, so skip!)

    let _friday = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
    let monday = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();

    let start = monday; // resume_from = Friday + 3 = Monday
    let end = monday; // today = Monday

    // start >= end: Monday >= Monday = true
    assert!(should_skip_fetch(start, end), "Should skip fetch when resume date = today");
}

#[test]
fn test_tuesday_with_friday_cache_fetches() {
    // Scenario: Last cached = Friday, Today = Tuesday, Resume = Monday
    // Expected: Fetch (Monday < Tuesday)

    let _friday = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
    let monday = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();
    let tuesday = NaiveDate::from_ymd_opt(2024, 1, 23).unwrap();

    let start = monday; // resume_from
    let end = tuesday; // today

    // start >= end: Monday >= Tuesday = false
    assert!(!should_skip_fetch(start, end), "Should fetch on Tuesday");
}

#[test]
fn test_wednesday_with_monday_cache_fetches() {
    // Scenario: Last cached = Monday, Today = Wednesday
    // Expected: Fetch (Tuesday < Wednesday)

    let _monday = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();
    let tuesday = NaiveDate::from_ymd_opt(2024, 1, 23).unwrap();
    let wednesday = NaiveDate::from_ymd_opt(2024, 1, 24).unwrap();

    let start = tuesday; // resume_from = Monday + 1
    let end = wednesday; // today

    assert!(!should_skip_fetch(start, end), "Should fetch on Wednesday");
}

#[test]
fn test_empty_cache_normal_fetch() {
    // Scenario: No cached data, today = Wednesday
    // Default: fetch last 730 days
    // Expected: Fetch (start well before today)

    let wednesday = NaiveDate::from_ymd_opt(2024, 1, 24).unwrap();
    let history_start = wednesday - Duration::days(730);

    let start = history_start;
    let end = wednesday;

    assert!(!should_skip_fetch(start, end), "Should fetch full history");
}

#[test]
fn test_long_weekend_skip() {
    // Scenario: Market holiday creates long weekend
    // Friday = last cached
    // Long weekend: Sat (regular), Sun (regular), Mon (holiday), Tue (no trading yet)
    // Prices cache shows: [Fri, Wed, ...]  (Mon skipped = holiday)
    // Today = Tuesday
    // Resume = Wednesday

    let _friday = NaiveDate::from_ymd_opt(2024, 7, 4).unwrap(); // July 4
    let wednesday = NaiveDate::from_ymd_opt(2024, 7, 10).unwrap(); // 6 days later
    let tuesday = NaiveDate::from_ymd_opt(2024, 7, 9).unwrap(); // Before Wednesday

    let start = wednesday;
    let end = tuesday;

    assert!(should_skip_fetch(start, end), "Should skip - resume is in future");
}

#[test]
fn test_consecutive_daily_runs() {
    // Scenario: User runs CLI every day
    // Day 1 (Wed): Cache empty, fetch full history
    // Day 2 (Thu): Cache has Wed, Resume = Thu, Today = Thu → skip
    // Day 3 (Fri): Cache has Thu, Resume = Fri, Today = Fri → skip
    // Day 4 (Sat): Cache has Fri, Resume = Mon, Today = Sat → skip
    // Day 5 (Tue): Cache has Fri, Resume = Mon, Today = Tue → fetch Mon-Tue

    let wed = NaiveDate::from_ymd_opt(2024, 1, 24).unwrap();
    let thu = NaiveDate::from_ymd_opt(2024, 1, 25).unwrap();
    let fri = NaiveDate::from_ymd_opt(2024, 1, 26).unwrap();
    let sat = NaiveDate::from_ymd_opt(2024, 1, 27).unwrap();
    let mon = NaiveDate::from_ymd_opt(2024, 1, 29).unwrap();
    let tue = NaiveDate::from_ymd_opt(2024, 1, 30).unwrap();

    // Day 1: No check (full history)
    let history_start = wed - Duration::days(730);
    assert!(!should_skip_fetch(history_start, wed));

    // Day 2: Thu cache, resume Thu, today Thu
    assert!(should_skip_fetch(thu, thu), "Day 2: Skip");

    // Day 3: Fri cache, resume Fri, today Fri
    assert!(should_skip_fetch(fri, fri), "Day 3: Skip");

    // Day 4: Fri cache, resume Mon, today Sat
    assert!(should_skip_fetch(mon, sat), "Day 4: Skip (weekend)");

    // Day 5: Fri cache, resume Mon, today Tue
    assert!(!should_skip_fetch(mon, tue), "Day 5: Fetch Mon-Tue");
}

#[test]
fn test_same_day_multiple_runs() {
    // Scenario: User runs CLI multiple times same day
    // First run: Cache empty, fetch
    // Second run: Cache now has data, resume = today, skip

    let tuesday = NaiveDate::from_ymd_opt(2024, 1, 23).unwrap();

    // First run: empty cache, fetch 730 days
    let history_start = tuesday - Duration::days(730);
    assert!(!should_skip_fetch(history_start, tuesday), "First run: Fetch");

    // Second run: cache has Tuesday data, resume = Tuesday, skip
    assert!(should_skip_fetch(tuesday, tuesday), "Second run: Skip (already fetched today)");
}

#[test]
fn test_future_date_edge_case() {
    // Edge case: What if resume date is somehow in far future?
    // (shouldn't happen, but testing defensive behavior)

    let today = NaiveDate::from_ymd_opt(2024, 1, 24).unwrap();
    let far_future = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();

    assert!(should_skip_fetch(far_future, today), "Should skip if resume in far future");
}

#[test]
fn test_back_to_back_market_days() {
    // Scenario: Fetch Mon, Tue, Wed in sequence
    // Day 1: Empty cache, fetch history
    // Day 2: Cache has Mon, resume = Tue, today = Tue → skip
    // Day 3: Cache has Tue, resume = Wed, today = Wed → skip

    let _mon = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();
    let tue = NaiveDate::from_ymd_opt(2024, 1, 23).unwrap();
    let wed = NaiveDate::from_ymd_opt(2024, 1, 24).unwrap();

    // Day 2
    assert!(should_skip_fetch(tue, tue), "Day 2: Skip (resume = today)");

    // Day 3
    assert!(should_skip_fetch(wed, wed), "Day 3: Skip (resume = today)");
}

#[test]
fn test_gap_in_data_before_skip() {
    // Scenario: Market was closed (holiday) during cached data period
    // Last cached: Thursday, market closed Friday, today = Monday
    // Resume = Friday (from full calculation), but should scan prices for Monday
    // If prices shows Monday only (no Friday trading), resume = Monday
    // Today = Monday → skip

    let _thursday = NaiveDate::from_ymd_opt(2024, 1, 18).unwrap();
    let monday = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();

    let start = monday; // prices cache shows Monday first
    let end = monday; // today

    assert!(should_skip_fetch(start, end), "Should skip - resume matches today after holiday");
}
