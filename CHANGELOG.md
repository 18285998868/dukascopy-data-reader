# Changelog

All notable changes follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-11-14

### Added

- `LoadReport` with requested/loaded/missing files, decoded/trimmed/invalid/
  out-of-bucket/duplicate row counts, gap summary and first/last bar time.
- Gap detection (`GapSummary`) over the returned series; gaps are reported,
  never filled.
- Bounded file planning: a request with both dates expands only its buckets
  and does not walk the symbol's history.
- Batched parallel decoding (64 files per batch) with per-batch merge.
- Python `load()` returning `(columns, report)`; `volume` is `uint32`.
- `config/00.example.toml`, `config/00.smoke.toml`, `examples/load.rs` and
  `scripts/check.sh`.

### Changed

- **Breaking:** archive path months are zero-based (`00` = January), matching
  Dukascopy's datafeed and the downloader mirror. Years and days stay
  one-based. July 2023 now lives in `.../2023/06/`.
- **Breaking:** configuration key `dir` is now `root`; the reader appends
  `dukascopy/{symbol}/` itself.
- **Breaking:** `load`/`load_columns` take `&DataConfig`; the old scan-and-parse
  module tree (`src/app/`) was replaced by `layout`, `load`, `decode`,
  `columns`, `gaps` and `report`.
- **Breaking:** a corrupt `.bi5` is a hard error carrying its path; previously
  the failure was not consistently surfaced with the file path.
- Missing files no longer abort a load; they are reported.
- `lzma-rs` replaced by `lzma-rust2`; `walkdir` removed.
- PyO3 and NumPy moved behind the default-off `python` feature.

### Fixed

- H1 files were decoded against a one-based month path, which shifted every
  monthly bucket by one month and dropped trailing bars.

## [0.1.0] - 2026-10-31

- Initial reader: scan the symbol tree, decode all `.bi5` files and return
  OHLCV columns.
