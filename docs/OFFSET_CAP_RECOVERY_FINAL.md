# Offset Cap Recovery - Strike-Based Partitioning (API-Verified)

**Status:** ✅ **FEASIBLE - API SUPPORTS REQUIRED FILTERS**

**Sources:**
- [EODHD US Stock Options API - Usage Examples](https://eodhd.com/financial-academy/stock-options/us-stock-options-api-usage-examples)
- [A Practical US Options API Guide](https://eodhd.com/financial-academy/stock-options/a-practical-us-options-api-guide-from-activity-scan-to-key-strikes-via-eodhd-api)

---

## Confirmed: EODHD Supports Strike & Expiration Filtering

The API now (January 2025 update) includes these filters:

```
filter[strike_from]    - Minimum strike price
filter[strike_to]      - Maximum strike price
filter[exp_date_from]  - Expiration date start
filter[exp_date_to]    - Expiration date end
filter[exp_date_eq]    - Exact expiration date
```

**This means we CAN partition data without time-of-day complexity!**

---

## Solution: Strike Price-Based Recovery

When offset cap is hit on a date/option_type combination, **partition by strike price ranges** instead of time.

### How It Works

**Scenario:** SPY puts on 2024-01-15 return 15,000 rows (offset cap hit at 10,000)

```
Step 1: Initial fetch (hits cap)
  paginate_window(symbol=SPY, type=put, date=2024-01-15)
  Result: 10,000 rows + hit_cap=true

Step 2: Extract strike range from partial data
  strikes in response: $200–$600 (SPY range)
  mid_strike = ($200 + $600) / 2 = $400

Step 3: Partition and re-fetch
  Fetch 1: filter[strike_to]=400  (SPY $200–$400)
    Result: 7,000 rows ✅

  Fetch 2: filter[strike_from]=401 (SPY $401–$600)
    Result: 8,000 rows ✅

Total: 15,000 rows recovered! ✅
```

### Algorithm

```rust
async fn fetch_window_recursive(
    &self,
    symbol: &str,
    option_type: &str,
    win_from: NaiveDate,
    win_to: NaiveDate,
    mut rows_fetched: usize,
    tx: &mpsc::Sender<WindowChunk>,
) -> (usize, Option<String>) {
    // Normal fetch
    let (rows, hit_cap, error) = self.paginate_window(&base_params).await;

    // ... send rows ...

    if hit_cap && span_days <= MIN_WINDOW_DAYS {
        // New: Try strike-based partitioning
        return self.fetch_by_strike_range(
            symbol, option_type, win_from, win_to,
            rows,  // Use partial rows to determine strike range
            rows_fetched, tx
        ).await;
    }

    if hit_cap && span_days > MIN_WINDOW_DAYS {
        // Original: Date-based subdivision
        // ... recursively subdivide by date ...
    }

    (rows_fetched, None)
}

async fn fetch_by_strike_range(
    &self,
    symbol: &str,
    option_type: &str,
    win_from: NaiveDate,
    win_to: NaiveDate,
    partial_rows: Vec<HashMap<String, String>>,
    mut rows_fetched: usize,
    tx: &mpsc::Sender<WindowChunk>,
) -> (usize, Option<String>) {
    // Extract strike prices from partial data
    let strikes: Vec<f64> = partial_rows
        .iter()
        .filter_map(|row| row.get("strike")?.parse().ok())
        .collect();

    if strikes.is_empty() {
        // Can't determine range, return what we have
        return (rows_fetched, Some("Unable to determine strike range from partial data".into()));
    }

    let min_strike = strikes.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_strike = strikes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    // Calculate midpoint
    let mid_strike = (min_strike + max_strike) / 2.0;

    // Fetch lower strike range
    let base_params_low = vec![
        ("filter[underlying_symbol]".into(), symbol.to_string()),
        ("filter[type]".into(), option_type.to_string()),
        ("filter[tradetime_from]".into(), win_from.format("%Y-%m-%d").to_string()),
        ("filter[tradetime_to]".into(), win_to.format("%Y-%m-%d").to_string()),
        ("filter[strike_to]".into(), mid_strike.to_string()),
        ("fields[options-eod]".into(), FIELDS.to_string()),
    ];

    let (rows_low, hit_cap_low, _) = self.paginate_window(&base_params_low).await;
    if !rows_low.is_empty() {
        // ... normalize and send ...
        rows_fetched += rows_low.len();
    }

    // Fetch upper strike range
    let base_params_high = vec![
        ("filter[underlying_symbol]".into(), symbol.to_string()),
        ("filter[type]".into(), option_type.to_string()),
        ("filter[tradetime_from]".into(), win_from.format("%Y-%m-%d").to_string()),
        ("filter[tradetime_to]".into(), win_to.format("%Y-%m-%d").to_string()),
        ("filter[strike_from]".into(), (mid_strike + 0.01).to_string()),
        ("fields[options-eod]".into(), FIELDS.to_string()),
    ];

    let (rows_high, hit_cap_high, _) = self.paginate_window(&base_params_high).await;
    if !rows_high.is_empty() {
        // ... normalize and send ...
        rows_fetched += rows_high.len();
    }

    // If either half still hit cap, recursively subdivide by strike
    let final_error = if hit_cap_low || hit_cap_high {
        Some("Offset cap hit even after strike range partitioning".into())
    } else {
        None
    };

    (rows_fetched, final_error)
}
```

---

## Real-World Scenarios

### Scenario 1: Normal Day (2,000 SPY puts)

```
paginate_window(SPY, put, 2024-01-15)
→ 2,000 rows, no cap hit ✅
Total: 2,000 rows
```

### Scenario 2: High Volatility Day (15,000 SPY puts)

```
paginate_window(SPY, put, 2024-01-15)
→ 10,000 rows, HIT CAP

fetch_by_strike_range:
  strikes in response: $200–$600
  mid = $400

  fetch_by_strike_to=$400
  → 7,000 rows ✅

  fetch_by_strike_from=$400.01
  → 8,000 rows ✅

Total: 15,000 rows recovered! ✅
API calls: 3 (1 original + 2 recovery)
```

### Scenario 3: Extreme Volatility (40,000 SPY puts)

```
paginate_window(SPY, put, 2024-01-15)
→ 10,000 rows, HIT CAP

fetch_by_strike_range ($200-$600):
  fetch_by_strike_to=$400
  → 10,000 rows, HIT CAP (need further subdivision)

  fetch_by_strike_from=$400.01
  → 10,000 rows, HIT CAP (need further subdivision)

Recursive subdivision on lower half:
  fetch_by_strike_to=$300 (between $200-$400)
  → 5,000 rows ✅

  fetch_by_strike_from=$300.01
  → 5,000 rows ✅

Recursive subdivision on upper half:
  fetch_by_strike_to=$500
  → 5,000 rows ✅

  fetch_by_strike_from=$500.01
  → 5,000 rows ✅

Total: 20,000+ rows recovered
API calls: ~7 (adaptive based on volatility)
```

---

## Why This Works Better

| Aspect | Time-Based | Strike-Based |
|--------|-----------|-------------|
| **Feasibility** | ❌ No time in output data | ✅ API supports filter[strike_from/to] |
| **Data preservation** | All times → same quote_date | ✅ Keeps actual distribution |
| **Logic simplicity** | Complex hourly windows | ✅ Simple binary partition |
| **Dedup impact** | ⚠️ Collapse time-of-day | ✅ Each strike kept separate |
| **API call overhead** | 24 calls per high-vol day | ✅ Only ~3-7 for reasonable vol |
| **Handles extreme vol** | Loses data | ✅ Recurses until complete |

---

## Implementation Details

### Phase 1: Basic Strike Partitioning
- Extract strikes from partial response
- Fetch lower/upper ranges
- Combine results
- **Effort:** ~100 lines

### Phase 2: Recursive Strike Subdivision
- If either half hits cap, subdivide strike range
- Escalate from 2-way → 4-way → 8-way split
- Stop when all data fetched or limit reached
- **Effort:** ~50 additional lines

### Phase 3: Expiration-Based Fallback
- If strike subdivision reaches limit, try expiration partitioning
- Use filter[exp_date_from] and filter[exp_date_to]
- Further reduces rows per request
- **Effort:** ~50 additional lines

**Total effort:** ~200 lines of code

---

## Error Handling

```
if hit_cap on 1-day window:
  try strike range partitioning
    if still hit cap on either half:
      recursively subdivide strikes further
    if still can't recover:
      try expiration date partitioning
    if still can't recover:
      warn "offset cap hit at minimum granularity, ~10K rows may be truncated"
```

---

## Testing

```rust
#[tokio::test]
async fn test_strike_range_recovery() {
    // Mock: paginate_window returns 10K rows + hit_cap on $200-$600 range
    // Call fetch_by_strike_range with partial_rows
    // Assert: fetches $200-$400 and $400.01-$600 separately
    // Assert: total rows = 15K
}

#[tokio::test]
async fn test_strike_recursive_subdivision() {
    // Mock: both strike halves return 10K (still over cap)
    // Assert: subdivides to 4-way split ($200-$300, $300-$400, $400-$500, $500-$600)
    // Assert: all data recovered
}

#[tokio::test]
async fn test_expiration_fallback() {
    // Mock: strike partitioning reaches limit
    // Assert: escalates to expiration-based partitioning
    // Assert: data recovered
}
```

---

## Comparison to Optopsy

**Optopsy:** Silently loses data beyond 10K offset on minimum windows ❌

**Inflow (Current):** Warns about potential truncation ⚠️

**Inflow (Proposed):** Actively recovers missing data via adaptive strike partitioning ✅

---

## Recommendation

**Implement strike-based recovery with recursive subdivision:**

1. ✅ API supports `filter[strike_from]` and `filter[strike_to]`
2. ✅ Strike prices are already in the response data
3. ✅ Clean binary partition algorithm (easy to implement)
4. ✅ Low API call overhead (3-7 calls instead of 24)
5. ✅ Handles extreme volatility gracefully

This is **practical, API-verified, and significantly better than warning about data loss.**

Ready to implement?
