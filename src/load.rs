//! Load orchestration: plan buckets, decode files, assemble columns.
//!
//! The pipeline is intentionally file-plan first: the requested date range is
//! expanded into the exact bucket paths, existence is checked per path, and
//! only present files are decoded. This keeps a bounded range independent of
//! how much history the symbol directory contains.
//!
//! Decoding is parallel in batches. Each batch is merged before the next one
//! starts, so peak memory stays close to the final column size instead of
//! holding every decoded file plus the output at once.

use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{NaiveDate, NaiveTime};
use rayon::prelude::*;

use crate::columns::{ColumnBuffer, KlineColumns};
use crate::config::DataConfig;
use crate::decode::{DecodedFile, decode_file};
use crate::error::{ReaderError, Result};
use crate::gaps::GapSummary;
use crate::layout::{self, BucketKey};
use crate::report::{LoadReport, MissingFile};

/// Files decoded per parallel batch. Large enough to saturate a CPU, small
/// enough that one batch's columns are negligible next to the final output.
const DECODE_BATCH: usize = 64;
/// Gap segments kept in the report (totals are always complete).
const MAX_GAP_SAMPLES: usize = 8;
/// Missing paths shown in the warning log (the report keeps all of them).
const MAX_MISSING_SAMPLES: usize = 5;

/// Columns plus the report describing how they were produced.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedData {
    columns: KlineColumns,
    report: LoadReport,
}

impl LoadedData {
    /// Decoded OHLCV columns.
    #[must_use]
    pub fn columns(&self) -> &KlineColumns {
        &self.columns
    }

    /// What was loaded, missing or cleaned.
    #[must_use]
    pub fn report(&self) -> &LoadReport {
        &self.report
    }

    /// Consume the result.
    #[must_use]
    pub fn into_parts(self) -> (KlineColumns, LoadReport) {
        (self.columns, self.report)
    }
}

/// Load one symbol × timeframe × price side, returning columns and a report.
///
/// # Errors
///
/// Returns [`ReaderError::Io`] when the symbol directory or a file cannot be
/// read, and [`ReaderError::CorruptFile`] when a `.bi5` file is malformed.
/// Missing files are not errors; they are listed in the report.
#[tracing::instrument(
    skip(config),
    fields(
        symbol = %config.symbol(),
        timeframe = %config.timeframe(),
        price_type = %config.price_type(),
    )
)]
pub fn load(config: &DataConfig) -> Result<LoadedData> {
    let started = Instant::now();
    let timeframe = config.timeframe();
    let symbol_dir = layout::symbol_dir(config.root(), config.symbol());
    layout::require_dir(&symbol_dir)?;
    // A fully bounded request expands its own buckets and must not enumerate
    // the symbol tree: history can be decades deep and is irrelevant here.
    let discovered = if config.start_date().is_some() && config.end_date().is_some() {
        Vec::new()
    } else {
        layout::existing_buckets(&symbol_dir, timeframe)?
    };
    let requested = resolve_requested(config, &discovered);

    let files: Vec<(BucketKey, PathBuf)> = requested
        .iter()
        .map(|&bucket| {
            (
                bucket,
                layout::bucket_file(&symbol_dir, config.price_type(), bucket),
            )
        })
        .collect();

    let presence: Vec<Presence> = files
        .par_iter()
        .map(|(_, path)| check_presence(path))
        .collect::<Result<Vec<_>>>()?;

    let mut present = Vec::with_capacity(presence.len());
    let mut missing = Vec::new();
    for ((bucket, path), state) in files.into_iter().zip(presence) {
        match state {
            Presence::Present => present.push((bucket, path)),
            Presence::Missing => missing.push(MissingFile::new(bucket, path)),
        }
    }

    tracing::info!(
        files = present.len(),
        missing = missing.len(),
        "planned dukascopy files"
    );

    let capacity = present.len().saturating_mul(max_rows_per_bucket(timeframe));
    let mut buffer = ColumnBuffer::with_capacity(capacity);
    let mut rows_invalid = 0usize;
    let mut rows_out_of_bucket = 0usize;
    for batch in present.chunks(DECODE_BATCH) {
        let decoded: Vec<DecodedFile> = batch
            .par_iter()
            .map(|(bucket, path)| decode_file(path, *bucket))
            .collect::<Result<Vec<_>>>()?;
        for file in decoded {
            rows_invalid += file.rows_invalid;
            rows_out_of_bucket += file.rows_out_of_bucket;
            buffer.append(file);
        }
    }

    let rows_decoded = buffer.len();
    let (trim_start, trim_end) = date_bounds_ms(config)?;
    let rows_trimmed = buffer.trim(trim_start, trim_end);
    buffer.sort_if_needed();
    let duplicates_removed = buffer.dedup();

    let gaps = GapSummary::scan(buffer.time(), timeframe.interval_ms(), MAX_GAP_SAMPLES);
    let first_bar_ms = buffer.time().first().copied();
    let last_bar_ms = buffer.time().last().copied();
    let columns = buffer.finish();

    let report = LoadReport {
        requested_files: requested.len(),
        loaded_files: present.len(),
        missing_files: missing,
        rows_decoded,
        rows_trimmed,
        rows_invalid,
        rows_out_of_bucket,
        duplicates_removed,
        gaps,
        first_bar_ms,
        last_bar_ms,
    };
    log_report(config, &report, started);
    Ok(LoadedData { columns, report })
}

/// Convenience wrapper when only the columns are needed.
///
/// # Errors
///
/// Same as [`load`].
pub fn load_columns(config: &DataConfig) -> Result<KlineColumns> {
    Ok(load(config)?.into_parts().0)
}

/// Presence of one planned file.
enum Presence {
    Present,
    Missing,
}

fn check_presence(path: &Path) -> Result<Presence> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(Presence::Present),
        Ok(_) => Err(ReaderError::corrupt(
            path,
            "path exists but is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Presence::Missing),
        Err(error) => Err(ReaderError::io(path, error)),
    }
}

/// Expand the request into buckets.
///
/// With no bounds, the buckets found on disk are used as-is. With one bound,
/// the other side is taken from the discovered range and every bucket in
/// between is requested, so holes become visible in the report. With both
/// bounds, `discovered` is empty and the range is expanded without touching
/// the archive tree.
fn resolve_requested(config: &DataConfig, discovered: &[BucketKey]) -> Vec<BucketKey> {
    let (start, end) = (config.start_date(), config.end_date());
    if start.is_none() && end.is_none() {
        return discovered.to_vec();
    }
    let start = start.or_else(|| discovered.first().map(|bucket| bucket.start()));
    let end = end.or_else(|| discovered.last().map(|bucket| bucket.start()));
    match (start, end) {
        (Some(start), Some(end)) => {
            layout::buckets_between(config.timeframe(), start, end).collect()
        }
        _ => Vec::new(),
    }
}

/// Inclusive `[start, end)` Unix-millisecond bounds for the configured dates.
fn date_bounds_ms(config: &DataConfig) -> Result<(Option<i64>, Option<i64>)> {
    let start = config.start_date().map(day_start_ms);
    let end = match config.end_date() {
        Some(date) => {
            let next = date.succ_opt().ok_or_else(|| {
                ReaderError::Integrity(format!("end_date {date} has no next calendar day"))
            })?;
            Some(day_start_ms(next))
        }
        None => None,
    };
    Ok((start, end))
}

fn day_start_ms(date: NaiveDate) -> i64 {
    date.and_time(NaiveTime::MIN).and_utc().timestamp_millis()
}

/// Upper bound of valid rows in one bucket; used only for pre-allocation.
const fn max_rows_per_bucket(timeframe: crate::types::Timeframe) -> usize {
    match timeframe {
        crate::types::Timeframe::M1 => 1_440,
        crate::types::Timeframe::H1 => 744,
        crate::types::Timeframe::D1 => 366,
    }
}

fn log_report(config: &DataConfig, report: &LoadReport, started: Instant) {
    if !report.missing_files.is_empty() {
        let samples: Vec<String> = report
            .missing_files
            .iter()
            .take(MAX_MISSING_SAMPLES)
            .map(ToString::to_string)
            .collect();
        tracing::warn!(
            symbol = %config.symbol(),
            timeframe = %config.timeframe(),
            price_type = %config.price_type(),
            missing = report.missing_files.len(),
            samples = ?samples,
            "dukascopy files are missing; re-run the downloader for these buckets"
        );
    }
    if report.rows_invalid > 0 || report.rows_out_of_bucket > 0 {
        tracing::warn!(
            rows_invalid = report.rows_invalid,
            rows_out_of_bucket = report.rows_out_of_bucket,
            "dukascopy rows dropped during decoding"
        );
    }
    if report.duplicates_removed > 0 {
        tracing::warn!(
            duplicates_removed = report.duplicates_removed,
            "duplicate bars removed by open_time (first kept)"
        );
    }
    if report.gaps.segments() > 0 {
        tracing::warn!(
            gap_segments = report.gaps.segments(),
            missing_bars = report.gaps.missing_bars(),
            misaligned_segments = report.gaps.misaligned_segments(),
            samples = ?report.gaps.samples(),
            "bar gaps detected; series kept as stored"
        );
    }
    tracing::info!(
        symbol = %config.symbol(),
        timeframe = %config.timeframe(),
        price_type = %config.price_type(),
        files = report.loaded_files,
        missing = report.missing_files.len(),
        rows = report.rows_returned(),
        elapsed = ?started.elapsed(),
        "loaded dukascopy candles"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PriceType, Timeframe};
    use std::io::Write;
    use tempfile::TempDir;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn day_offset(day: u32) -> u32 {
        day * 86_400
    }

    fn record(day: u32, price: u32, volume: u32) -> [u8; 24] {
        let mut rec = [0u8; 24];
        rec[0..4].copy_from_slice(&day_offset(day).to_be_bytes());
        rec[4..8].copy_from_slice(&price.to_be_bytes());
        rec[8..12].copy_from_slice(&price.to_be_bytes());
        rec[12..16].copy_from_slice(&(price - 1).to_be_bytes());
        rec[16..20].copy_from_slice(&(price + 1).to_be_bytes());
        rec[20..24].copy_from_slice(&volume.to_be_bytes());
        rec
    }

    fn write_bi5(path: &Path, records: &[[u8; 24]]) {
        let options = lzma_rust2::LzmaOptions::with_preset(6);
        let mut writer = lzma_rust2::LzmaWriter::new_use_header(
            Vec::new(),
            &options,
            Some((records.len() * 24) as u64),
        )
        .unwrap();
        for rec in records {
            writer.write_all(rec).unwrap();
        }
        let bytes = writer.finish().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn d1_path(root: &Path, year: i32) -> PathBuf {
        root.join(format!("dukascopy/EURUSD/{year}/BID_candles_day_1.bi5"))
    }

    /// `month` is the logical calendar month; the archive path is zero-based.
    fn h1_path(root: &Path, year: i32, month: u32) -> PathBuf {
        root.join(format!(
            "dukascopy/EURUSD/{year}/{:02}/BID_candles_hour_1.bi5",
            month - 1
        ))
    }

    fn ask_h1_path(root: &Path, year: i32, month: u32) -> PathBuf {
        root.join(format!(
            "dukascopy/EURUSD/{year}/{:02}/ASK_candles_hour_1.bi5",
            month - 1
        ))
    }

    fn config(root: &Path) -> DataConfig {
        DataConfig::new(root, "EURUSD", Timeframe::D1).unwrap()
    }

    #[test]
    fn loads_only_the_requested_price_side() {
        let tmp = TempDir::new().unwrap();
        write_bi5(&d1_path(tmp.path(), 2025), &[record(0, 100_000, 7)]);
        let ask = tmp
            .path()
            .join("dukascopy/EURUSD/2025/ASK_candles_day_1.bi5");
        write_bi5(&ask, &[record(0, 200_000, 9)]);

        let bid = load(&config(tmp.path())).unwrap();
        assert_eq!(bid.columns().open(), [1.0]);
        assert_eq!(bid.columns().volume(), [7]);
        assert_eq!(bid.report().loaded_files(), 1);
        assert_eq!(bid.report().duplicates_removed(), 0);

        let ask = config(tmp.path()).with_price_type(PriceType::Ask);
        let loaded = load(&ask).unwrap();
        assert_eq!(loaded.columns().open(), [2.0]);
        assert_eq!(loaded.columns().volume(), [9]);
    }

    #[test]
    fn date_range_expands_to_buckets_and_reports_missing() {
        let tmp = TempDir::new().unwrap();
        write_bi5(&d1_path(tmp.path(), 2024), &[record(0, 100_000, 1)]);
        write_bi5(&d1_path(tmp.path(), 2025), &[record(0, 100_000, 2)]);

        let cfg = config(tmp.path())
            .with_date_range(Some(date(2023, 1, 1)), Some(date(2025, 12, 31)))
            .unwrap();
        let loaded = load(&cfg).unwrap();
        assert_eq!(loaded.report().requested_files(), 3);
        assert_eq!(loaded.report().loaded_files(), 2);
        assert_eq!(loaded.report().missing_files().len(), 1);
        assert_eq!(
            loaded.report().missing_files()[0].bucket().to_string(),
            "2023"
        );
        assert_eq!(loaded.columns().len(), 2);
        assert!(!loaded.report().is_complete());
    }

    #[test]
    fn missing_files_do_not_abort_the_load() {
        let tmp = TempDir::new().unwrap();
        write_bi5(&d1_path(tmp.path(), 2025), &[record(0, 100_000, 1)]);
        let cfg = config(tmp.path())
            .with_date_range(Some(date(2024, 1, 1)), Some(date(2025, 12, 31)))
            .unwrap();
        let loaded = load(&cfg).unwrap();
        assert_eq!(loaded.report().loaded_files(), 1);
        assert_eq!(loaded.report().missing_files().len(), 1);
        assert_eq!(loaded.columns().len(), 1);
    }

    #[test]
    fn corrupt_file_is_a_hard_error_with_path() {
        let tmp = TempDir::new().unwrap();
        let path = d1_path(tmp.path(), 2025);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not lzma").unwrap();
        let err = load(&config(tmp.path())).unwrap_err();
        match err {
            ReaderError::CorruptFile { path: reported, .. } => assert_eq!(reported, path),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn date_range_trims_rows_inside_a_partial_bucket() {
        let tmp = TempDir::new().unwrap();
        let records: Vec<[u8; 24]> = (0..10).map(|day| record(day, 100_000, 1)).collect();
        write_bi5(&d1_path(tmp.path(), 2025), &records);
        let cfg = config(tmp.path())
            .with_date_range(Some(date(2025, 1, 3)), Some(date(2025, 1, 4)))
            .unwrap();
        let loaded = load(&cfg).unwrap();
        assert_eq!(loaded.columns().len(), 2);
        assert_eq!(loaded.report().rows_decoded(), 10);
        assert_eq!(loaded.report().rows_trimmed(), 8);
        assert_eq!(
            loaded.columns().time(),
            [1_735_862_400_000, 1_735_948_800_000]
        );
    }

    #[test]
    fn duplicate_timestamps_are_removed_after_sorting() {
        let tmp = TempDir::new().unwrap();
        write_bi5(
            &d1_path(tmp.path(), 2025),
            &[
                record(1, 100_000, 1),
                record(0, 100_000, 2),
                record(0, 100_000, 3),
            ],
        );
        let loaded = load(&config(tmp.path())).unwrap();
        assert_eq!(loaded.columns().time().len(), 2);
        assert_eq!(loaded.report().duplicates_removed(), 1);
        // Stable sort keeps the first record for the duplicated timestamp.
        assert_eq!(loaded.columns().volume(), [2, 1]);
    }

    #[test]
    fn unsorted_file_is_sorted_globally() {
        let tmp = TempDir::new().unwrap();
        write_bi5(
            &d1_path(tmp.path(), 2025),
            &[
                record(2, 100_000, 1),
                record(0, 100_000, 2),
                record(1, 100_000, 3),
            ],
        );
        let loaded = load(&config(tmp.path())).unwrap();
        assert!(loaded.columns().time().windows(2).all(|w| w[0] < w[1]));
        assert_eq!(loaded.columns().volume(), [2, 3, 1]);
    }

    #[test]
    fn gaps_are_reported() {
        let tmp = TempDir::new().unwrap();
        write_bi5(
            &d1_path(tmp.path(), 2025),
            &[
                record(0, 100_000, 1),
                record(1, 100_000, 1),
                record(4, 100_000, 1),
            ],
        );
        let loaded = load(&config(tmp.path())).unwrap();
        assert_eq!(loaded.report().gaps().segments(), 1);
        assert_eq!(loaded.report().gaps().missing_bars(), 2);
    }

    #[test]
    fn empty_symbol_directory_returns_an_empty_result() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("dukascopy/EURUSD")).unwrap();
        let loaded = load(&config(tmp.path())).unwrap();
        assert!(loaded.columns().is_empty());
        assert_eq!(loaded.report().requested_files(), 0);
        assert!(loaded.report().is_complete());
    }

    #[test]
    fn nonexistent_symbol_directory_is_an_io_error() {
        let tmp = TempDir::new().unwrap();
        let err = load(&config(tmp.path())).unwrap_err();
        match err {
            ReaderError::Io { path, .. } => {
                assert_eq!(path, tmp.path().join("dukascopy/EURUSD"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn symbol_path_that_is_a_file_is_an_io_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("dukascopy")).unwrap();
        std::fs::write(tmp.path().join("dukascopy/EURUSD"), b"x").unwrap();
        let err = load(&config(tmp.path())).unwrap_err();
        match err {
            ReaderError::Io { path, .. } => {
                assert_eq!(path, tmp.path().join("dukascopy/EURUSD"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn fully_bounded_range_expands_without_discovery() {
        let tmp = TempDir::new().unwrap();
        let cfg = config(tmp.path())
            .with_date_range(Some(date(2024, 1, 1)), Some(date(2025, 12, 31)))
            .unwrap();
        let requested = resolve_requested(&cfg, &[]);
        assert_eq!(
            requested
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["2024", "2025"]
        );
    }

    #[test]
    fn one_sided_range_uses_the_discovered_edge() {
        let tmp = TempDir::new().unwrap();
        let cfg = config(tmp.path())
            .with_date_range(Some(date(2024, 1, 1)), None)
            .unwrap();
        let discovered = [
            BucketKey::new(Timeframe::D1, date(2023, 1, 1)),
            BucketKey::new(Timeframe::D1, date(2025, 1, 1)),
        ];
        let requested = resolve_requested(&cfg, &discovered);
        assert_eq!(
            requested
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["2024", "2025"]
        );
    }

    #[test]
    fn hourly_buckets_list_missing_months() {
        let tmp = TempDir::new().unwrap();
        write_bi5(&h1_path(tmp.path(), 2025, 1), &[record(0, 100_000, 1)]);
        write_bi5(&ask_h1_path(tmp.path(), 2025, 1), &[record(0, 200_000, 2)]);
        let cfg = DataConfig::new(tmp.path(), "EURUSD", Timeframe::H1)
            .unwrap()
            .with_date_range(Some(date(2025, 1, 1)), Some(date(2025, 3, 31)))
            .unwrap();
        let loaded = load(&cfg).unwrap();
        assert_eq!(loaded.report().requested_files(), 3);
        assert_eq!(loaded.report().loaded_files(), 1);
        assert_eq!(loaded.report().missing_files().len(), 2);
        assert_eq!(loaded.columns().open(), [1.0]);
        assert_eq!(loaded.columns().time(), [1_735_689_600_000]);
    }
}
