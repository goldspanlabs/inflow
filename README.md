# Inflow

**Inflow** is a standalone CLI tool for downloading and caching market data (options chains from EODHD and OHLCV prices from Yahoo Finance) to populate `~/.optopsy/cache/` Parquet files — independently of the optopy-mcp MCP server.

## Features

- 🚀 **Concurrent downloads** — Download multiple symbols in parallel with configurable concurrency limits
- 📊 **Options chains** — Fetch historical options data from EODHD with automatic pagination and recursive window subdivision
- 💹 **OHLCV prices** — Download historical price data from Yahoo Finance
- 💾 **Incremental writes** — Each data chunk is atomically written to prevent corruption from interruptions
- 🔄 **Resume support** — Downloads can be resumed from the last cached date
- ⚡ **Rate limiting** — Built-in adaptive rate limiting respects API quotas
- 🛟 **Error recovery** — Exponential backoff on transient failures; non-fatal errors don't block other symbols

## Installation

### From Source

```bash
git clone https://github.com/goldspanlabs/inflow.git
cd inflow
cargo build --release
./target/release/inflow --help
```

### Requirements

- Rust 1.70+
- `EODHD_API_KEY` environment variable (for options downloads; optional)
- Yahoo Finance API (no API key required)

## Configuration

Inflow loads configuration from environment variables and `.env` files:

1. `~/.env` (home directory)
2. `./.env` (current directory)
3. Environment variables

### Configuration Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `DATA_ROOT` | `~/.optopsy/cache` | Root cache directory |
| `EODHD_API_KEY` | (none) | EODHD API key; if unset, EODHD provider is disabled |
| `EODHD_RATE_LIMIT` | `10` | EODHD rate limit in requests/second |

### Example `.env` File

```env
# Cache directory
DATA_ROOT=~/.optopsy/cache

# EODHD API (obtain from https://eodhd.com)
EODHD_API_KEY=your_api_key_here

# Rate limit (requests per second)
EODHD_RATE_LIMIT=10
```

## Usage

### Show Configuration

```bash
inflow config
```

Displays resolved configuration with status indicators:

```
Inflow Configuration:

  DATA_ROOT: /Users/user/.optopsy/cache
  EODHD_API_KEY: ✓ configured
  EODHD_RATE_LIMIT: 10 req/sec
```

### Download Options Chains

Fetch historical options data from EODHD:

```bash
# Download recent options for SPY
inflow download options SPY

# Download multiple symbols
inflow download options SPY QQQ IWM

# Download from specific date (defaults to ~2 years of history)
inflow download options SPY --from 2024-01-01

# Adjust concurrency (default: 4)
inflow download options SPY QQQ --concurrency 8
```

**Note:** Requires `EODHD_API_KEY` environment variable.

### Download Prices

Fetch OHLCV price data from Yahoo Finance:

```bash
# Download 1 year of daily prices for SPY (default)
inflow download prices SPY

# Download 5 years of historical data
inflow download prices SPY --period 5y

# Available periods: 1mo, 3mo, 6mo, 1y, 5y, max
inflow download prices SPY --period max

# Download multiple symbols in parallel
inflow download prices SPY QQQ IWM --concurrency 8
```

### Download Both

Fetch both options and prices for symbols:

```bash
inflow download all SPY QQQ --from 2024-01-01 --period 1y --concurrency 4
```

### Show Cache Status

Display summary of cached data:

```bash
inflow status
```

Output example:

```
📊 Options Cache:

Symbol │ Rows  │ Size (MB) │ Date Range
───────┼───────┼───────────┼──────────────────
QQQ    │ 18942 │    1.24   │ 2022-03-16 → 2024-03-04
SPY    │ 24156 │    1.58   │ 2022-03-16 → 2024-03-04

💹 Prices Cache:

Symbol │ Rows  │ Size (MB) │ Date Range
───────┼───────┼───────────┼──────────────────
QQQ    │  253  │    0.08   │ 2023-03-04 → 2024-03-01
SPY    │  253  │    0.08   │ 2023-03-04 → 2024-03-01
```

## Architecture

### Producer–Consumer Pipeline

Inflow uses a **concurrent producer–consumer architecture**:

1. **Producers** — One worker per (provider, symbol) pair
   - Acquire a semaphore slot to limit concurrency
   - Call the provider's download method
   - Send data chunks via an async MPSC channel

2. **Consumer** — Single writer task
   - Receives data chunks from all producers
   - Merges with existing cache (for options)
   - Deduplicates based on key columns
   - Atomically writes to cache (via temporary file rename)

### Data Providers

#### EODHD Provider
- **Category:** options
- **Data:** Historical options chains (bid, ask, Greeks, etc.)
- **Window strategy:** ~30-day rolling windows with recursive subdivision on offset cap
- **Rate limiting:** Adaptive throttle based on `X-RateLimit-Remaining` header; exponential backoff on 429/5xx
- **Resume:** Automatically fetches from latest cached date

#### Yahoo Finance Provider
- **Category:** prices
- **Data:** OHLCV + Adjusted Close + Volume
- **Periods:** 1mo, 3mo, 6mo, 1y, 5y, max
- **Resume:** Overwrites entire history (simpler than options)

### Cache Layout

```
~/.optopsy/cache/
├── options/
│   ├── SPY.parquet
│   ├── QQQ.parquet
│   └── ...
└── prices/
    ├── SPY.parquet
    ├── QQQ.parquet
    └── ...
```

Each Parquet file contains:
- **Options:** `underlying_symbol`, `option_type`, `expiration`, `quote_date`, `strike`, Greeks, bid/ask/last, etc.
- **Prices:** `date`, `open`, `high`, `low`, `close`, `adjclose`, `volume`

### Atomic Writes

All Parquet writes are **atomic** via temporary file rename:

```rust
let tmp_path = path.with_extension("parquet.tmp");
// Write to temp file
ParquetWriter::new(File::create(&tmp_path)?).finish(df)?;
// Atomic rename to final path
std::fs::rename(&tmp_path, path)?;
```

This prevents data corruption if the process is interrupted during write.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All symbols succeeded |
| 1 | Partial failure or runtime error |
| 2 | Configuration error |

## Development

### Running Tests

```bash
cargo test
```

### Building Release Binary

```bash
cargo build --release
./target/release/inflow --help
```

### Dependency Versions

**Critical compatibility:**
- `polars = "0.53"` — must match optopy-mcp for Parquet format compatibility
- `yahoo_finance_api = "4.1"` — must match optopy-mcp

### Code Structure

```
src/
├── main.rs              # Entry point; CLI dispatch
├── error.rs             # Error types and exit codes
├── config.rs            # Configuration loading from env
├── cli.rs               # Clap derive CLI parser
├── cache/
│   ├── mod.rs           # Re-exports
│   ├── store.rs         # CacheStore; atomic writes
│   └── scan.rs          # Cache file inspection
├── pipeline/
│   ├── mod.rs           # Re-exports
│   ├── types.rs         # WindowChunk, DownloadResult, etc.
│   ├── orchestrator.rs  # Pipeline orchestration
│   ├── producer.rs      # Per-symbol worker logic
│   └── consumer.rs      # Merge and write logic
├── providers/
│   ├── mod.rs           # DataProvider trait; factory
│   ├── eodhd.rs         # EODHD provider (~550 lines)
│   └── yahoo.rs         # Yahoo provider (~150 lines)
└── commands/
    ├── mod.rs           # Re-exports
    ├── download.rs      # Download command handler
    ├── status.rs        # Status command handler
    └── config.rs        # Config command handler
```

## Performance Tips

1. **Increase concurrency** for multiple symbols:
   ```bash
   inflow download all SPY QQQ IWM --concurrency 16
   ```

2. **Start with smaller date ranges** to test configuration:
   ```bash
   inflow download options SPY --from 2024-01-01
   ```

3. **Monitor rate limits** — if you see 429 errors, reduce concurrency or EODHD_RATE_LIMIT

4. **Use `inflow status`** to verify cache before analysis

## Troubleshooting

### "EODHD_API_KEY is invalid or expired"
- Check your API key at https://eodhd.com
- Ensure it's exported in your environment: `export EODHD_API_KEY=...`
- Test with `inflow config`

### "Offset cap hit; data may be incomplete"
- EODHD API has a 10K offset limit per request
- Inflow automatically subdivides windows when this happens
- If it persists on 1-day windows, data exists but is truncated

### "No data returned for {symbol}"
- Symbol may not exist on Yahoo Finance
- Try a different period (e.g., `--period 1mo`)
- Check that the symbol is valid

### Cache files not updating
- Run `inflow status` to check current cached data
- Ensure `DATA_ROOT` directory is writable
- Try downloading a single symbol first: `inflow download prices SPY`

## Integration with optopy-mcp

After downloading data with **inflow**, the optopy-mcp MCP server can directly read the Parquet files from `~/.optopsy/cache/` without re-downloading:

```bash
# Populate cache with inflow
inflow download all SPY QQQ IWM

# Start optopy-mcp; it will use the cached files
# (optopy-mcp will only download missing or expired data)
```

## License

MIT

## Contributing

Contributions welcome! Please open an issue or pull request at https://github.com/goldspanlabs/inflow.
