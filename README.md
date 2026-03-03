# Inflow

**Inflow** is a standalone CLI tool for downloading and caching market data (options chains from EODHD and OHLCV prices from Yahoo Finance) to populate `~/.optopsy/cache/` Parquet files — independently of the optopy-mcp MCP server.

> [!NOTE]
> This project is currently in a pre-release state. We are iterating quickly, which means breaking changes to the API and configuration may occur without prior notice. Use in production environments at your own risk.

## Features

- 🚀 **Concurrent downloads** — Download multiple symbols in parallel with configurable concurrency
- 📊 **Options chains** — Fetch historical options data from EODHD
- 💹 **OHLCV prices** — Download historical price data from Yahoo Finance
- 🔄 **Resume support** — Only fetches data newer than what's already cached
- ⚡ **Rate limiting** — Built-in adaptive rate limiting respects API quotas
- 🛟 **Error recovery** — Transient failures don't block other symbols
- 🔍 **Data quality checks** — Validate cached data for gaps, duplicates, schema issues, and outliers

## Quick Start

```bash
git clone https://github.com/goldspanlabs/inflow.git
cd inflow

# Create a ~/.env file with your EODHD API key (for options data)
echo "EODHD_API_KEY=your_api_key_here" >> ~/.env
cargo run -- download options SPY

# Download prices (no API key needed) and options
cargo run -- download prices SPY
cargo run -- status
cargo run -- check
```

## Installation

### From Source

```bash
git clone https://github.com/goldspanlabs/inflow.git
cd inflow
cargo build --release

# Optionally copy to a directory on your PATH
cp ./target/release/inflow /usr/local/bin/
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

### Delete Cached Data

Remove cached options, prices, or both for one or more symbols:

```bash
# Delete cached data for a single symbol (interactive prompt)
inflow delete SPY

# Delete cached data for multiple symbols
inflow delete SPY QQQ IWM
```

For each symbol, you'll be prompted to choose whether to delete options, prices, or both.

### Check Data Quality

Validate cached data for gaps, duplicates, schema issues, and outliers:

```bash
# Check all cached symbols
inflow check

# Check specific symbols
inflow check --symbols SPY QQQ
```

**Options checks:** trading day gaps (cross-referenced against prices calendar), duplicate rows, schema validation, null/outlier detection (zero prices, invalid deltas), and delta coverage (verifies each date has full call+put strike spread).

**Prices checks:** schema validation, null/outlier detection (zero or negative prices), and gap detection (consecutive missing trading days > 3).

## Cache Layout

```
~/.optopsy/cache/
├── options/
│   ├── SPY.parquet
│   └── ...
└── prices/
    ├── SPY.parquet
    └── ...
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
- Ensure it's set in `~/.env` or exported in your shell: `export EODHD_API_KEY=your_key`
- Verify with `inflow config`

### "No data returned for {symbol}"
- Symbol may not exist on the provider
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
