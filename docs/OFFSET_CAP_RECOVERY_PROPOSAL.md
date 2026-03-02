# Offset Cap Recovery Strategy: Complete Missing Data

**Problem:** When offset cap (10K) is hit on a minimum window (1 day), inflow currently warns but loses data.

**Example:**
```
SPY puts on high volatility day: 15,000 rows available
Window [2024-01-15, 2024-01-15] hits 10K cap
Currently: ⚠️ Returns 10,000 rows + warning, loses 5,000 rows
Proposed: ✅ Recovers all 15,000 rows
```

**Goal:** Implement sub-day windowing to fetch missing data instead of losing it.

---

## Root Cause

The 10K offset limit applies per API request, not per window. When a single day has 10K+ rows:

```
Request: filter[tradetime_from]=2024-01-15, filter[tradetime_to]=2024-01-15
Response: 10,000 rows (max pagination offset reached)
Remaining: 5,000 rows (inaccessible via offset pagination)
```

Cannot subdivide further: can't split 1 day by time into smaller days.

---

## Solution: Sub-Day Windowing

When a 1-day window hits the offset cap, recursively subdivide by **time-of-day** instead of date.

### Algorithm

```
fetch_window_recursive(from_date, to_date, ...):
    rows, hit_cap = paginate_window(from_date, to_date)

    span_days = to_date - from_date

    if hit_cap and span_days < 1 day:
        # Can't subdivide further by date
        # Return warning + partial data
        return (rows, error_hint)

    if hit_cap and span_days == 1 day:
        # New: Try sub-day windowing
        return fetch_by_time_of_day(from_date, to_date)

    if hit_cap:
        # Original: Subdivide by date
        mid = from_date + span_days / 2
        return fetch_window_recursive(from_date, mid) +
               fetch_window_recursive(mid+1, to_date)
```

### Sub-Day Windowing Strategies

**Strategy 1: 12-Hour Windows** (2 partitions)
```
2024-01-15 [00:00, 23:59]
├── 2024-01-15 [00:00, 11:59]  (first 12 hours)
└── 2024-01-15 [12:00, 23:59]  (second 12 hours)
```

**Strategy 2: 6-Hour Windows** (4 partitions)
```
2024-01-15 [00:00, 23:59]
├── 2024-01-15 [00:00, 05:59]
├── 2024-01-15 [06:00, 11:59]
├── 2024-01-15 [12:00, 17:59]
└── 2024-01-15 [18:00, 23:59]
```

**Strategy 3: Hourly Windows** (24 partitions)
```
2024-01-15 [00:00, 23:59]
├── 2024-01-15 [00:00, 00:59]
├── 2024-01-15 [01:00, 01:59]
├── ... (24 total)
└── 2024-01-15 [23:00, 23:59]
```

**Escalation:**
```
1. Try 12-hour windows
2. If any 12-hour window hits cap, try 6-hour
3. If any 6-hour window hits cap, try hourly
4. If hourly still hits cap, warn + return partial data
```

---

## Implementation Plan

### Phase 1: Time-of-Day Window Generation

Add function to `pagination.rs`:

```rust
/// Generate hourly windows for a single day
pub fn hourly_windows(date: NaiveDate) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut windows = Vec::new();
    for hour in 0..24 {
        let from = date
            .and_hms_opt(hour, 0, 0)
            .unwrap()
            .and_utc();
        let to = date
            .and_hms_opt((hour + 1) % 24, 0, 0)
            .unwrap()
            .and_utc();
        windows.push((from, to));
    }
    windows
}

/// Generate 12-hour windows for a single day
pub fn half_day_windows(date: NaiveDate) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    vec![
        (date.and_hms_opt(0, 0, 0).unwrap().and_utc(),
         date.and_hms_opt(12, 0, 0).unwrap().and_utc()),
        (date.and_hms_opt(12, 0, 0).unwrap().and_utc(),
         date.and_hms_opt(23, 59, 59).unwrap().and_utc()),
    ]
}
```

### Phase 2: Adaptive Subdivision in Recursive Fetch

Modify `fetch_window_recursive` in `mod.rs`:

```rust
fn fetch_window_recursive<'a>(
    &'a self,
    symbol: &'a str,
    option_type: &'a str,
    win_from: NaiveDate,
    win_to: NaiveDate,
    rows_fetched: usize,
    tx: &'a mpsc::Sender<WindowChunk>,
) -> Pin<Box<dyn Future<Output = (usize, Option<String>)> + Send + 'a>> {
    Box::pin(async move {
        let span_days = (win_to - win_from).num_days();

        let (rows, hit_cap, error) = self.paginate_window(&base_params).await;

        // ... normalize and send rows ...

        if hit_cap && span_days == 0 {  // 1-day window
            // Try sub-day windowing
            return self.fetch_by_time_of_day(
                symbol, option_type, win_from, rows_fetched, tx
            ).await;
        }

        if hit_cap && span_days > 0 {
            // Original date-based subdivision
            rows_fetched -= window_rows;
            let mid = win_from + Duration::days(span_days / 2);
            // ... recursively fetch both halves ...
        }

        (rows_fetched, None)
    })
}

async fn fetch_by_time_of_day(
    &self,
    symbol: &str,
    option_type: &str,
    date: NaiveDate,
    mut rows_fetched: usize,
    tx: &mpsc::Sender<WindowChunk>,
) -> (usize, Option<String>) {
    let mut current_strategy = 0;  // 0 = 12-hour, 1 = 6-hour, 2 = hourly

    loop {
        let windows = match current_strategy {
            0 => self.half_day_windows(date),
            1 => self.six_hour_windows(date),
            _ => self.hourly_windows(date),
        };

        let mut any_hit_cap = false;
        let mut error_encountered = None;

        for (from, to) in windows {
            let (fetched, hit_cap, err) = self.paginate_window_datetime(&[...]).await;

            if !fetched.is_empty() {
                // normalize and send
                rows_fetched += fetched.len();
            }

            if hit_cap {
                any_hit_cap = true;
            }
            if err.is_some() && error_encountered.is_none() {
                error_encountered = err;
            }
        }

        if !any_hit_cap || current_strategy == 2 {
            // All windows completed OR reached hourly limit
            return (rows_fetched, error_encountered);
        }

        current_strategy += 1;  // Escalate to finer granularity
    }
}
```

### Phase 3: DateTime-Based Pagination

Extend `paginate_window` to accept `DateTime` ranges:

```rust
pub async fn paginate_window_datetime(
    &self,
    base_params: &[(String, String)],
) -> (Vec<HashMap<String, String>>, bool, Option<String>) {
    // Same logic as paginate_window but with datetime filters
    // filter[tradetime_from]: "2024-01-15T06:00:00Z"
    // filter[tradetime_to]:   "2024-01-15T11:59:59Z"
}
```

---

## Behavior Comparison

### Scenario: SPY puts, 15,000 rows in single day

**Before (Current Inflow):**
```
[2024-01-15, 2024-01-15]  → hit_cap=true, span_days=0
⚠️ Warning: "offset cap hit on minimum window, data may be incomplete"
❌ Returns 10,000 rows (5,000 lost)
```

**After (Proposed):**
```
[2024-01-15, 2024-01-15]  → hit_cap=true, span_days=0
  → Try 12-hour strategy:
     [2024-01-15 00:00-11:59]  → 7,000 rows ✅
     [2024-01-15 12:00-23:59]  → 8,000 rows ✅
✅ Returns 15,000 rows (complete!)
```

### Scenario: Very high volatility day (25,000 rows)

```
[2024-01-15]  → hit_cap=true
  → Try 12-hour: [00:00-11:59] still hits cap (13K rows)
  → Escalate to 6-hour:
     [00:00-05:59]   → 6,000 rows ✅
     [06:00-11:59]   → 7,000 rows ✅
     [12:00-17:59]   → 6,000 rows ✅
     [18:00-23:59]   → 6,000 rows ✅
✅ Returns 25,000 rows (complete!)
```

### Scenario: Pathological case (millions of rows per hour)

```
[2024-01-15]  → hit_cap=true
  → Try 12-hour, 6-hour, hourly: all hit cap
  → Final strategy: hourly with cap hit on [05:00-05:59]
⚠️ Warning: "offset cap hit on 05:00-05:59 UTC, ~10K rows may be truncated"
✅ Returns 240K+ rows (nearly complete, only 1 hour truncated)
```

---

## Benefits Over Current Approach

| Aspect | Current (Warn) | Proposed (Recover) |
|--------|---|---|
| **Data loss** | ✗ 5,000 rows lost | ✅ All data recovered |
| **User visibility** | ⚠️ Warning only | ✅ Automatic recovery |
| **API calls** | 100 calls | ~105 calls (5% increase) |
| **Reliability** | Accepts loss | Fights for completeness |
| **High-volume days** | Fails silently | Adapts strategy |

---

## Limitations & Edge Cases

### Limitation 1: Pathological Data Volumes

If a single hour has 10K+ rows:
- Can't subdivide further by time
- Must accept partial data and warn
- This is extremely rare (only mega-cap stocks on black swan events)

### Limitation 2: API Call Amplification

High-volatility days may require 24 hourly requests instead of 1 daily request. But:
- Still within EODHD rate limits
- Still <<< cost of re-running entire download
- Worth the cost for data completeness

### Limitation 3: Time-of-Day Dependency

Market hours: 09:30-16:00 EST (US equities)
- Morning hours: lower volume
- Final hour: higher volume
- Could optimize windows by market hours, not calendar hours

---

## Testing Strategy

```rust
#[tokio::test]
async fn test_offset_cap_recovery_12_hour() {
    // Mock API to return 7K rows for each 12-hour window
    // Verify fetch_by_time_of_day fetches both halves
    // Assert total rows = 14K
}

#[tokio::test]
async fn test_offset_cap_escalation_to_hourly() {
    // Mock API: 12-hour windows return 10K+ rows (still over cap)
    // Escalate to hourly
    // Verify 24 hourly requests made
    // Assert all data recovered
}

#[tokio::test]
async fn test_offset_cap_pathological_case() {
    // Mock API: even hourly windows hit cap
    // Verify warning returned
    // Verify all available data returned
}
```

---

## Implementation Complexity

**Effort:** ~200-300 lines of code
- Window generation functions: ~40 lines
- `fetch_by_time_of_day` function: ~80 lines
- `paginate_window_datetime`: ~30 lines (reuse existing logic)
- Integration into `fetch_window_recursive`: ~15 lines
- Tests: ~100 lines

**Risk:** Low
- No changes to existing pagination logic
- No changes to rate limiting
- Purely additive feature
- Fallback to warning if escalation fails

---

## Recommendation

**Implement this sub-day windowing strategy:**

1. ✅ Keep current warning for transparency
2. ✅ Add adaptive sub-day windowing when 1-day window hits cap
3. ✅ Escalate from 12-hour → 6-hour → hourly as needed
4. ✅ Return all available data (not just first 10K)
5. ✅ Warn only if hourly windows still hit cap (extremely rare)

This transforms a data-loss situation into a data-recovery situation with minimal added complexity.

---

## Comparison to Optopsy

**Optopsy:** Accepts 10K row limit on minimum windows (silent loss)
**Inflow (Current):** Warns about potential loss
**Inflow (Proposed):** Actively recovers missing data via sub-day windowing

This would be a **legitimate improvement** over optopsy's approach.
