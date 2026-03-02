# Inflow Code Review & Refactoring Plan

## Executive Summary

**Analysis Date:** March 2, 2026
**Codebase Size:** 2,103 lines across 19 modules
**Issues Found:** 15 major issues
**Estimated Reduction:** ~300 lines (14%) through consolidation
**Priority:** Critical (DRY violations), High (organization), Medium (patterns)

---

## Critical Issues (Fix Immediately)

### 1. ❌ Date Conversion Logic Duplicated 3 Times
**Severity:** CRITICAL | **Files:** 3 | **Lines:** 9 duplicated lines

**Locations:**
- `src/providers/eodhd.rs:686-693` (closure in `extract_date_range`)
- `src/providers/yahoo.rs:168-175` (closure in `extract_date_range`)
- `src/cache/scan.rs:111-117` (function `date_scalar_to_naive`)

**Code:** Identical logic with magic constant `719_163` (Excel date offset)

**Impact:**
- Hard to maintain (3 copies to update)
- Magic numbers scattered
- Inconsistent naming (closure vs function)

**Fix:** Extract to `src/utils/date.rs`

---

### 2. ❌ `extract_date_range()` Duplicated 2 Times
**Severity:** CRITICAL | **Files:** 2 | **Lines:** ~10 duplicated

**Locations:**
- `src/providers/eodhd.rs:683-698` (uses `"quote_date"`)
- `src/providers/yahoo.rs:165-180` (uses `"date"`)

**Issue:** Nearly identical functions with one-line difference

**Fix:** Single parameterized function in utils

---

### 3. ❌ JSON Value Parsing Duplicated
**Severity:** HIGH | **File:** 1 | **Lines:** 12 duplicated

**Location:** `src/providers/eodhd.rs`
- `parse_compact_rows():567-590`
- `parse_standard_rows():592-617`

**Duplication:** Value-to-string conversion logic identical in both

**Fix:** Extract `json_value_to_string()` utility

---

### 4. ❌ DEDUP_COLS Constant Duplicated
**Severity:** HIGH | **Files:** 2 | **Sync Risk:** YES

**Locations:**
- `src/providers/eodhd.rs:73-79`
- `src/pipeline/consumer.rs:12-18`

**Risk:** If one is updated, the other silently breaks

**Fix:** Move to `src/utils/constants.rs`

---

## High-Priority Issues (Should Fix)

### 5. ❌ DataFrame Concat Args Duplicated
**Severity:** HIGH | **File:** 1 | **Lines:** 2 instances

**Location:** `src/pipeline/consumer.rs`
- Lines 78-83: `concat()` with UnionArgs
- Lines 95-100: Identical `concat()` call

**Issue:** Magic values for union strategy

**Fix:** Constant or helper function

---

### 6. ❌ Provider Filtering Logic Duplicated
**Severity:** HIGH | **File:** 1 | **Lines:** 6

**Location:** `src/commands/download.rs`
- Lines 33-37: Filter for "options"
- Lines 58-62: Filter for "prices"

**Pattern:** Identical filter-clone-collect pattern

**Fix:** Extract `filter_providers_by_category()` helper

---

### 7. ❌ EODHD Provider is 698 Lines
**Severity:** MEDIUM | **Module:** Over-weighted | **Maintainability:** Hard

**Structure:** Monolithic file mixing:
- HTTP logic (throttled_get, retry)
- Pagination (paginate_window, monthly_windows)
- Parsing (3 functions)
- Normalization (normalize_rows)
- Public API

**Recommendation:** Split into submodules:
```
src/providers/eodhd/
├── mod.rs          (~200 lines: provider impl)
├── http.rs         (~100 lines: HTTP + rate limit)
├── pagination.rs   (~100 lines: windows + recursive fetch)
├── parsing.rs      (~150 lines: JSON parsing + normalization)
└── types.rs        (~50 lines: API types)
```

---

### 8. ❌ Inconsistent Error Handling Patterns
**Severity:** MEDIUM | **Impact:** Code clarity

**Patterns found:**
- `(rows, hit_cap, error)` tuple returns (EODHD)
- `Result<T>` with `?` operator (Yahoo)
- Error wrapping in DownloadResult (Producer)

**Recommendation:** Standardize patterns documentation

---

### 9. ❌ Consumer.rs is Complex (125 lines, 60-line function)
**Severity:** MEDIUM | **Testability:** Hard

**Issues:**
- `write_options()` is 60 lines with nested logic
- Three `if let` statements in sequence
- Repeated `concat()` calls
- Mixed concerns (merge + dedup + sort)

**Fix:** Break into smaller functions

---

## Medium-Priority Issues

### 10. ❌ Cache Path Selection Duplicated
**Locations:**
- `src/cache/store.rs:120-131` (validate_symbol method)
- `src/commands/status.rs:50-54` (path selection)

**Fix:** Add method `CacheStore::get_path(category, symbol)`

---

### 11. ❌ Table Printing Boilerplate (2 instances)
**Files:** `src/commands/download.rs`, `src/commands/status.rs`

**Issue:** Both implement similar table creation/printing pattern

**Fix:** Table builder utility or template

---

### 12. ❌ Hardcoded Date Column Names
**Scattered:** 5+ locations
- `"quote_date"` for options
- `"date"` for prices

**Fix:** Constants `OPTIONS_DATE_COLUMN`, `PRICES_DATE_COLUMN`

---

### 13. ❌ Unused Parameter: `_rate_limit_per_sec`
**Location:** `src/providers/eodhd.rs:115`

**Issue:** Parameter accepted but never used. Configuration hardcoded.

**Fix:** Implement dynamic rate limiting OR remove parameter

---

## Code Organization Issues

### Current Structure
```
src/
├── main.rs              (70 lines)
├── error.rs             (20 lines)
├── config.rs            (65 lines)
├── cli.rs               (60 lines)
├── cache/
│   ├── mod.rs           (2 lines)
│   ├── store.rs         (150 lines)
│   └── scan.rs          (120 lines)
├── pipeline/
│   ├── mod.rs           (6 lines)
│   ├── types.rs         (100 lines)
│   ├── orchestrator.rs  (75 lines)
│   ├── producer.rs      (35 lines)
│   └── consumer.rs      (125 lines)
├── providers/
│   ├── mod.rs           (50 lines)
│   ├── eodhd.rs         (698 lines) ⚠️ OVER-WEIGHTED
│   └── yahoo.rs         (180 lines)
└── commands/
    ├── mod.rs           (6 lines)
    ├── download.rs      (160 lines)
    ├── status.rs        (85 lines)
    └── config.rs        (25 lines)
```

### Recommended Structure
```
src/
├── main.rs
├── error.rs
├── config.rs
├── cli.rs
├── utils/              ⭐ NEW MODULE
│   ├── mod.rs
│   ├── date.rs         (consolidate date logic)
│   ├── json.rs         (consolidate JSON parsing)
│   ├── constants.rs    (shared constants)
│   └── tables.rs       (table building)
├── cache/
│   ├── mod.rs
│   ├── store.rs
│   └── scan.rs
├── pipeline/
│   ├── mod.rs
│   ├── types.rs
│   ├── orchestrator.rs
│   ├── producer.rs
│   └── consumer.rs
├── providers/
│   ├── mod.rs
│   ├── eodhd/          ⭐ SPLIT INTO SUBMODULES
│   │   ├── mod.rs
│   │   ├── http.rs
│   │   ├── pagination.rs
│   │   ├── parsing.rs
│   │   └── types.rs
│   └── yahoo.rs
└── commands/
    ├── mod.rs
    ├── download.rs
    ├── status.rs
    └── config.rs
```

---

## Refactoring Roadmap

### Phase 1: Quick Wins (1-2 hours)
- [x] Extract date utilities → `src/utils/date.rs`
- [x] Extract JSON parsing → `src/utils/json.rs`
- [x] Move constants → `src/utils/constants.rs`
- [ ] Remove unused parameter
- **Result:** ~100 lines removed, 0 new bugs

### Phase 2: Medium Refactors (2-4 hours)
- [ ] Extract provider filtering
- [ ] Simplify consumer.rs
- [ ] Add cache path method
- [ ] Create table builder utility
- **Result:** ~80 lines removed, better separation

### Phase 3: Large Refactor (4-8 hours)
- [ ] Split EODHD into submodules
- [ ] Standardize error handling
- [ ] Add unit tests for utilities
- **Result:** ~150+ lines organized, better testability

### Phase 4: Polish (1-2 hours)
- [ ] Update imports throughout
- [ ] Add documentation
- [ ] Integration testing
- **Result:** Clean codebase, ~300 lines removed

---

## Metrics Summary

| Metric | Before | After | Improvement |
|--------|--------|-------|-------------|
| Total Lines | 2,103 | ~1,800 | -14% |
| EODHD Module | 698 | ~400 | -43% |
| Consumer Lines | 125 | ~80 | -36% |
| Duplicate Lines | ~50 | 0 | -100% |
| Cyclomatic Complexity | Med | Low | Better |
| Testability | Hard | Easy | Better |
| Documentation | Minimal | Good | Better |

---

## Testing Strategy

### Before Refactoring
```bash
cargo build
cargo test
```

### After Each Phase
```bash
cargo build --release
# Verify binary works
./target/release/inflow config
./target/release/inflow status
```

### Commit Strategy
- Separate commit per extracted utility
- One commit per module restructure
- Test before each commit

---

## High-Risk Changes

⚠️ **EODHD Provider Restructure** is highest risk (Phase 3)
- Split 698-line file into 5 modules
- Requires careful import management
- Solution: Do in small commits, test between each

✅ **Utils Extraction** is lowest risk (Phase 1)
- Create new files, update imports
- No logic changes
- Easy to verify: old + new code should be identical

---

## Notes

1. **No architectural changes** — all refactoring is cosmetic/organizational
2. **Binary compatibility maintained** — no public API changes
3. **Performance neutral** — extractions compile to same code
4. **Testing-friendly** — smaller modules are easier to unit test
5. **Documentation opportunity** — add module-level docs after restructure
