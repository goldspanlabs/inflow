# Recursive Subdivision Logic: Inflow vs Optopsy

**Status:** ✅ **INFLOW IMPLEMENTS MORE ROBUST VERSION**

**Focus:** How the provider handles the 10K offset cap by subdividing windows into smaller time ranges.

---

## Overview

When the EODHD API returns a 422 error (or detects 10K offset reached), both implementations:
1. Undo the partial row count (because subdivision will re-fetch)
2. Split the window into two halves
3. Recursively fetch each half
4. Merge results

**Critical requirement:** Avoid data loss when cap is hit on a minimum-size window.

---

## Side-by-Side Comparison

### Optopsy

```python
def _fetch_window_recursive(self, symbol, option_type, win_from, win_to, rows_fetched):
    rows, hit_cap, error = self._paginate_window(base_params)

    if error:
        return rows_fetched, error  # Skip window, continue

    if rows:
        self._normalize_and_save_window(rows, symbol)
        rows_fetched += len(rows)

    if hit_cap and span_days > 1:  # MIN_WINDOW_DAYS = 1
        rows_fetched -= len(rows)  # Undo partial count
        mid = from_dt + timedelta(days=span_days // 2)
        rows_fetched, _ = self._fetch_window_recursive(..., win_from, str(mid), ...)
        rows_fetched, _ = self._fetch_window_recursive(..., str(mid+1d), win_to, ...)

    return rows_fetched, None  # Always success, even if error occurred
```

### Inflow

```rust
pub fn fetch_window_recursive<'a>(...) -> Pin<Box<dyn Future<...>>> {
    Box::pin(async move {
        let (rows, hit_cap, error) = self.paginate_window(&base_params).await;

        let window_rows = rows.len();
        if !rows.is_empty() {
            normalize_rows(&rows) → send via tx
            rows_fetched += window_rows;
        }

        if let Some(ref err_msg) = error {
            return (rows_fetched, error);  // Skip window, continue
        }

        // ⚠️ IMPROVED: Explicit minimum window handling
        if hit_cap && span_days <= MIN_WINDOW_DAYS {
            return (
                rows_fetched,
                Some(format!("offset cap hit on minimum window, data may be incomplete"))
            );
        }

        if hit_cap {
            rows_fetched -= window_rows;  // Undo partial count

            let mid = win_from + Duration::days(span_days / 2);

            // First half
            let (fetched, first_err) = self.fetch_window_recursive(...).await;
            rows_fetched = fetched;

            // Second half
            let (fetched, second_err) = self.fetch_window_recursive(...).await;
            rows_fetched = fetched;

            // ⚠️ IMPROVED: Error propagation from recursive calls
            return (rows_fetched, first_err.or(second_err));
        }

        (rows_fetched, None)
    })
}
```

---

## Key Differences

### 1. Minimum Window Error Handling ⚠️

**Scenario:** Window is 1 day (minimum), but API returns 10K+ rows → offset cap hit

**Optopsy:**
```python
if hit_cap and span_days > 1:
    # Only subdivide if span_days > 1
    # If span_days == 1 (minimum), no subdivision attempted
    # Data past 10K offset is SILENTLY LOST ❌
```

**Issue:** If a 1-day window has 10K+ rows, optopsy returns partial data without warning.

**Example:**
```
Window: 2024-01-15 to 2024-01-15 (1 day, span_days = 0)
API returns: 10,000 rows + "more exists beyond offset 10K"
Optopsy: Returns 10,000 rows as complete (data loss)
Inflow: Returns error "offset cap hit on minimum window, data may be incomplete"
```

**Inflow:**
```rust
if hit_cap && span_days <= MIN_WINDOW_DAYS {
    return (rows_fetched, Some(
        "offset cap hit on minimum window, data may be incomplete"
    ));
}
```

**Verdict: ⚠️ INFLOW SAFER**
- Explicitly detects and warns about potential truncation
- Surfaces the issue to user (not silent failure)
- Allows retry with different parameters

---

### 2. Error Propagation ⚠️

**Scenario:** Recursive call encounters an error (network, normalization, etc.)

**Optopsy:**
```python
rows_fetched, _ = self._fetch_window_recursive(..., win_from, str(mid), ...)
#                 ↑ Underscore ignores error from first half
rows_fetched, _ = self._fetch_window_recursive(..., str(mid+1d), win_to, ...)
#                 ↑ Underscore ignores error from second half

return rows_fetched, None  # Returns success regardless of errors ❌
```

**Issue:** If either half encounters an error (network timeout, normalization failure), it's silently discarded.

**Example:**
```
First half fetch: Network timeout → error "connection reset"
Optopsy ignores it → returns (rows_fetched, None) ← Wrong!
User thinks download succeeded when first half failed
```

**Inflow:**
```rust
let (fetched, first_err) = self.fetch_window_recursive(...).await;
rows_fetched = fetched;

let (fetched, second_err) = self.fetch_window_recursive(...).await;
rows_fetched = fetched;

return (rows_fetched, first_err.or(second_err));  // Propagate error ✅
```

**Verdict: ⚠️ INFLOW MORE ROBUST**
- Captures errors from both recursive calls
- Propagates first error encountered (non-silent)
- User is informed of partial failures

---

## Execution Flow Comparison

### When offset cap is hit on a normal window (>1 day)

Both:
1. ✅ Add rows to fetched count
2. ✅ Undo partial count (because subdivision will re-fetch)
3. ✅ Calculate midpoint: `mid = start + (span_days / 2)`
4. ✅ Recursively fetch `[start, mid]`
5. ✅ Recursively fetch `[mid+1, end]`
6. ✅ Return total rows fetched

### When offset cap is hit on a minimum window (≤1 day)

| Step | Optopsy | Inflow |
|------|---------|--------|
| 1. Detect cap hit | ✅ `hit_cap = True` | ✅ `hit_cap = True` |
| 2. Check minimum | ❌ No explicit check | ✅ Explicit check: `span_days <= 1` |
| 3. Subdivision | ✗ Skipped (would be divide-by-zero anyway) | ✗ Skipped with error |
| 4. Return | ✅ `(rows, None)` - silent | ⚠️ `(rows, error_msg)` - explicit |
| 5. Data status | ❓ Unclear if truncated | ✅ Clear: "may be incomplete" |

---

## Midpoint Calculation Correctness

**Both use integer division (floor):**

```
span_days = 30
mid = start + (30 / 2) = start + 15

Window: [start, start+15]  (16 days)
Window: [start+16, end]    (15 days)
```

**Verification:** Gap-free coverage ✅
- First window goes from start to mid (inclusive)
- Second window starts at mid+1 (no gap, no overlap)
- Both implementations do this correctly

---

## Real-World Scenario

### Case: SPY puts on high volatility day

```
Date: 2024-01-15 (major earnings day)
Window: [2024-01-15, 2024-01-15]  (1 day, span_days = 0)
Rows available: 15,000 (high IV, many strikes)
API limit: 10,000 rows per offset

Optopsy behavior:
  - Fetches 10,000 rows
  - Detects hit_cap = true
  - Checks: span_days > 1? NO (0 > 1 = false)
  - No subdivision
  - Returns (10000, None) ← Silently truncated! ❌

Inflow behavior:
  - Fetches 10,000 rows
  - Detects hit_cap = true
  - Checks: span_days <= MIN_WINDOW_DAYS? YES (0 <= 1 = true)
  - Returns error: "offset cap hit on minimum window, data may be incomplete"
  - User sees warning in logs, knows data is incomplete
  - Can retry or adjust parameters ✅
```

---

## Error Handling Scenario

### Case: Network timeout during recursive fetch

```
First half: Win [start, mid]
  - Fetch succeeds: 5,000 rows
  - Normalize succeeds
  - Send to consumer: success

Second half: Win [mid+1, end]
  - Network timeout connecting to EODHD
  - Returns: (5000, Some("timeout"))

Optopsy behavior:
  - Ignores second_err with underscore
  - Returns (total_rows, None) ← Wrong! ❌
  - Caller thinks both halves succeeded
  - Logs show no indication of timeout

Inflow behavior:
  - Captures: second_err = Some("timeout")
  - Returns (total_rows, second_err)
  - Caller sees error, knows second half failed
  - Logs show timeout warning
  - Can retry or handle appropriately ✅
```

---

## Summary of Improvements

| Issue | Optopsy | Inflow | Impact |
|-------|---------|--------|--------|
| **Minimum window overflow** | Silent truncation | Explicit error | Data integrity warning |
| **Error propagation** | Errors discarded | Errors propagated | User awareness |
| **Minimum window math** | ✅ Correct | ✅ Correct | Same |
| **Midpoint calculation** | ✅ Correct | ✅ Correct | Same |
| **Gap-free coverage** | ✅ Verified | ✅ Verified | Same |

---

## Conclusion

**Inflow's recursive subdivision logic is feature-complete AND more robust than optopsy:**

### ✅ Core Algorithm (100% Match)
- Same midpoint calculation
- Same recursive structure
- Same row counting logic
- Same "undo partial count" pattern

### 🚀 Safety Improvements
1. **Minimum window detection:** Warns when offset cap hit on 1-day window (potential truncation)
2. **Error propagation:** Returns errors from recursive calls instead of silently discarding

### Recommendation
**The extra checks in inflow are GOOD PRACTICE:**
- They don't change correctness (Optopsy also handles minimum windows, but implicitly via loop termination)
- They make issues visible (error messages instead of silent failures)
- They enable better debugging and retry logic

**Inflow's version is production-ready and safer for edge cases.**
