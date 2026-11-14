//! Dukascopy archive layout: calendar buckets and file paths.
//!
//! This module is the single source of truth for the on-disk convention
//! produced by `dukascopy-data-downloader`:
//!
//! ```text
//! {root}/dukascopy/{symbol}/{YYYY}/[{MM}/[{DD}/]]{BID|ASK}_candles_{min_1|hour_1|day_1}.bi5
//! ```
//!
//! `D1` is one file per year, `H1` one per month and `M1` one per day. Paths
//! are built from the requested interval instead of walking the whole symbol
//! tree, so a bounded request touches only the files it needs.
//!
//! **The path month is zero-based** (`00` = January, `11` = December), because
//! the local mirror follows Dukascopy's datafeed layout. Days and years stay
//! one-based. [`BucketKey`] itself stores logical calendar dates; only path
//! rendering and parsing use the zero-based month.

use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};

use crate::error::{ReaderError, Result};
use crate::types::{PriceType, Symbol, Timeframe};

/// Directory below the configured root that mirrors Dukascopy's datafeed.
pub(crate) const DATA_PREFIX: &str = "dukascopy";

/// Absolute directory holding one symbol's archive.
#[must_use]
pub(crate) fn symbol_dir(root: &Path, symbol: &Symbol) -> PathBuf {
    root.join(DATA_PREFIX).join(symbol.as_str())
}

/// Ensure `path` exists and is a directory.
///
/// A bounded request never enumerates the archive, so this check is the only
/// filesystem access that proves the configured root and symbol are usable.
pub(crate) fn require_dir(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path).map_err(|e| ReaderError::io(path, e))?;
    if metadata.is_dir() {
        return Ok(());
    }
    Err(ReaderError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "path exists but is not a directory",
        ),
    })
}

/// One calendar bucket, normalised to its first day.
///
/// The bucket knows its timeframe, so it can render the directory components
/// and validate that decoded timestamps belong to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BucketKey {
    timeframe: Timeframe,
    start: NaiveDate,
}

impl BucketKey {
    /// Build the bucket of `timeframe` containing `date`.
    #[must_use]
    pub fn new(timeframe: Timeframe, date: NaiveDate) -> Self {
        let start = match timeframe {
            Timeframe::D1 => NaiveDate::from_ymd_opt(date.year(), 1, 1),
            Timeframe::H1 => NaiveDate::from_ymd_opt(date.year(), date.month(), 1),
            Timeframe::M1 => Some(date),
        }
        .unwrap_or(date);
        Self { timeframe, start }
    }

    /// Timeframe whose calendar this bucket belongs to.
    #[must_use]
    pub const fn timeframe(self) -> Timeframe {
        self.timeframe
    }

    /// First calendar day of the bucket.
    #[must_use]
    pub const fn start(self) -> NaiveDate {
        self.start
    }

    /// Last calendar day covered by the bucket (inclusive).
    #[must_use]
    pub fn end(self) -> NaiveDate {
        match self.timeframe {
            Timeframe::D1 => {
                NaiveDate::from_ymd_opt(self.start.year(), 12, 31).unwrap_or(self.start)
            }
            Timeframe::H1 => {
                let (year, month) = if self.start.month() == 12 {
                    (self.start.year().checked_add(1), 1)
                } else {
                    (Some(self.start.year()), self.start.month() + 1)
                };
                year.and_then(|y| NaiveDate::from_ymd_opt(y, month, 1))
                    .and_then(|d| d.pred_opt())
                    .unwrap_or(self.start)
            }
            Timeframe::M1 => self.start,
        }
    }

    /// Next bucket on the same calendar, if representable.
    #[must_use]
    pub fn next(self) -> Option<Self> {
        let start = match self.timeframe {
            Timeframe::D1 => self
                .start
                .year()
                .checked_add(1)
                .and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1)),
            Timeframe::H1 => {
                let (year, month) = if self.start.month() == 12 {
                    (self.start.year().checked_add(1)?, 1)
                } else {
                    (self.start.year(), self.start.month() + 1)
                };
                NaiveDate::from_ymd_opt(year, month, 1)
            }
            Timeframe::M1 => self.start.succ_opt(),
        }?;
        Some(Self {
            timeframe: self.timeframe,
            start,
        })
    }

    /// Midnight UTC of the bucket's first day, in Unix milliseconds.
    #[must_use]
    pub fn start_ms(self) -> Option<i64> {
        self.start
            .and_hms_opt(0, 0, 0)
            .map(|dt| dt.and_utc().timestamp_millis())
    }

    /// Exclusive upper bound of the bucket in Unix milliseconds.
    #[must_use]
    pub fn end_exclusive_ms(self) -> Option<i64> {
        self.next().and_then(Self::start_ms)
    }

    /// Render the path month: the archive counts months from zero.
    fn path_month(self) -> u32 {
        // INVARIANT: `NaiveDate` months are always 1..=12.
        self.start.month().saturating_sub(1)
    }

    /// Filename of this bucket for one price side.
    #[must_use]
    pub fn file_name(self, price_type: PriceType) -> String {
        format!("{}_{}.bi5", price_type.prefix(), self.timeframe.file_stem())
    }

    fn push_dir(self, path: &mut PathBuf) {
        path.push(format!("{:04}", self.start.year()));
        if self.timeframe != Timeframe::D1 {
            path.push(format!("{:02}", self.path_month()));
        }
        if self.timeframe == Timeframe::M1 {
            path.push(format!("{:02}", self.start.day()));
        }
    }
}

impl fmt::Display for BucketKey {
    /// Renders the archive path, including its zero-based month.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}", self.start.year())?;
        if self.timeframe != Timeframe::D1 {
            write!(f, "/{:02}", self.path_month())?;
        }
        if self.timeframe == Timeframe::M1 {
            write!(f, "/{:02}", self.start.day())?;
        }
        Ok(())
    }
}

/// Build the absolute `.bi5` path for `bucket` below `symbol_dir`.
#[must_use]
pub(crate) fn bucket_file(symbol_dir: &Path, price_type: PriceType, bucket: BucketKey) -> PathBuf {
    let mut path = symbol_dir.to_path_buf();
    bucket.push_dir(&mut path);
    path.push(bucket.file_name(price_type));
    path
}

/// Lazily yield every bucket of `timeframe` overlapping `[start, end]`.
///
/// The bucket containing `start` is yielded first. An inverted range yields
/// nothing.
pub(crate) fn buckets_between(
    timeframe: Timeframe,
    start: NaiveDate,
    end: NaiveDate,
) -> impl Iterator<Item = BucketKey> {
    let seed = (start <= end).then(|| BucketKey::new(timeframe, start));
    std::iter::successors(seed, move |bucket| {
        bucket.next().filter(|next| next.start() <= end)
    })
}

/// Discover the buckets that actually have a directory on disk.
///
/// Only directories matching the timeframe's depth are considered; stray
/// files and non-numeric directories are ignored. The result is sorted.
pub(crate) fn existing_buckets(symbol_dir: &Path, timeframe: Timeframe) -> Result<Vec<BucketKey>> {
    let mut buckets = Vec::new();
    for (year, year_path) in numeric_dirs(symbol_dir)? {
        let Ok(year) = i32::try_from(year) else {
            continue;
        };
        let Some(year_start) = NaiveDate::from_ymd_opt(year, 1, 1) else {
            continue;
        };
        match timeframe {
            Timeframe::D1 => buckets.push(BucketKey {
                timeframe,
                start: year_start,
            }),
            Timeframe::H1 => {
                for (month_index, _) in numeric_dirs(&year_path)? {
                    let Some(month) = logical_month(month_index) else {
                        tracing::debug!(year, month_index, "ignoring invalid zero-based month");
                        continue;
                    };
                    if let Some(start) = NaiveDate::from_ymd_opt(year, month, 1) {
                        buckets.push(BucketKey { timeframe, start });
                    } else {
                        tracing::debug!(year, month, "ignoring invalid month directory");
                    }
                }
            }
            Timeframe::M1 => {
                for (month_index, month_path) in numeric_dirs(&year_path)? {
                    let Some(month) = logical_month(month_index) else {
                        tracing::debug!(year, month_index, "ignoring invalid zero-based month");
                        continue;
                    };
                    for (day, _) in numeric_dirs(&month_path)? {
                        if let Some(start) = NaiveDate::from_ymd_opt(year, month, day) {
                            buckets.push(BucketKey { timeframe, start });
                        } else {
                            tracing::debug!(year, month, day, "ignoring invalid day directory");
                        }
                    }
                }
            }
        }
    }
    buckets.sort_unstable();
    Ok(buckets)
}

/// Convert a zero-based path month into a calendar month.
fn logical_month(month_index: u32) -> Option<u32> {
    month_index.checked_add(1).filter(|month| *month <= 12)
}

/// List immediate subdirectories whose names are `u32`, sorted by value.
fn numeric_dirs(dir: &Path) -> Result<Vec<(u32, PathBuf)>> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| ReaderError::io(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| ReaderError::io(dir, e))?;
        let file_type = entry
            .file_type()
            .map_err(|e| ReaderError::io(entry.path(), e))?;
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(value) = name.parse::<u32>() else {
            continue;
        };
        out.push((value, entry.path()));
    }
    out.sort_unstable_by_key(|(value, _)| *value);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn sym(s: &str) -> Symbol {
        Symbol::new(s).unwrap()
    }

    #[test]
    fn bucket_normalises_to_its_start() {
        assert_eq!(
            BucketKey::new(Timeframe::D1, date(2025, 6, 15)).start(),
            date(2025, 1, 1)
        );
        assert_eq!(
            BucketKey::new(Timeframe::H1, date(2025, 6, 15)).start(),
            date(2025, 6, 1)
        );
        assert_eq!(
            BucketKey::new(Timeframe::M1, date(2025, 6, 15)).start(),
            date(2025, 6, 15)
        );
    }

    #[test]
    fn bucket_end_is_the_last_day() {
        assert_eq!(
            BucketKey::new(Timeframe::D1, date(2025, 6, 15)).end(),
            date(2025, 12, 31)
        );
        assert_eq!(
            BucketKey::new(Timeframe::H1, date(2024, 2, 5)).end(),
            date(2024, 2, 29)
        );
        assert_eq!(
            BucketKey::new(Timeframe::M1, date(2025, 6, 15)).end(),
            date(2025, 6, 15)
        );
    }

    #[test]
    fn bucket_next_rolls_over_year() {
        assert_eq!(
            BucketKey::new(Timeframe::D1, date(2025, 6, 1))
                .next()
                .unwrap()
                .start(),
            date(2026, 1, 1)
        );
        assert_eq!(
            BucketKey::new(Timeframe::H1, date(2025, 12, 1))
                .next()
                .unwrap()
                .start(),
            date(2026, 1, 1)
        );
        assert_eq!(
            BucketKey::new(Timeframe::M1, date(2024, 2, 29))
                .next()
                .unwrap()
                .start(),
            date(2024, 3, 1)
        );
    }

    #[test]
    fn display_uses_the_zero_based_month() {
        assert_eq!(
            BucketKey::new(Timeframe::D1, date(2025, 6, 15)).to_string(),
            "2025"
        );
        assert_eq!(
            BucketKey::new(Timeframe::H1, date(2025, 6, 15)).to_string(),
            "2025/05"
        );
        assert_eq!(
            BucketKey::new(Timeframe::M1, date(2007, 1, 2)).to_string(),
            "2007/00/02"
        );
        assert_eq!(
            BucketKey::new(Timeframe::H1, date(2025, 12, 1)).to_string(),
            "2025/11"
        );
    }

    #[test]
    fn bucket_file_builds_all_three_depths() {
        let base = Path::new("/data/dukascopy/EURUSD");
        let d1 = BucketKey::new(Timeframe::D1, date(2025, 6, 15));
        assert_eq!(
            bucket_file(base, PriceType::Bid, d1),
            Path::new("/data/dukascopy/EURUSD/2025/BID_candles_day_1.bi5")
        );
        let h1 = BucketKey::new(Timeframe::H1, date(2025, 6, 15));
        assert_eq!(
            bucket_file(base, PriceType::Ask, h1),
            Path::new("/data/dukascopy/EURUSD/2025/05/ASK_candles_hour_1.bi5")
        );
        let m1 = BucketKey::new(Timeframe::M1, date(2007, 1, 2));
        assert_eq!(
            bucket_file(base, PriceType::Bid, m1),
            Path::new("/data/dukascopy/EURUSD/2007/00/02/BID_candles_min_1.bi5")
        );
    }

    #[test]
    fn buckets_between_collapses_and_rolls_over() {
        let d1: Vec<_> = buckets_between(Timeframe::D1, date(2023, 6, 1), date(2025, 6, 1))
            .map(|b| b.to_string())
            .collect();
        assert_eq!(d1, ["2023", "2024", "2025"]);

        let h1: Vec<_> = buckets_between(Timeframe::H1, date(2024, 11, 15), date(2025, 2, 3))
            .map(|b| b.to_string())
            .collect();
        assert_eq!(h1, ["2024/10", "2024/11", "2025/00", "2025/01"]);

        let m1: Vec<_> = buckets_between(Timeframe::M1, date(2024, 12, 30), date(2025, 1, 2))
            .map(|b| b.to_string())
            .collect();
        assert_eq!(m1, ["2024/11/30", "2024/11/31", "2025/00/01", "2025/00/02"]);
    }

    #[test]
    fn inverted_range_yields_nothing() {
        assert_eq!(
            buckets_between(Timeframe::D1, date(2025, 1, 5), date(2025, 1, 3)).count(),
            0
        );
    }

    #[test]
    fn start_and_end_bounds_are_utc_midnight() {
        let bucket = BucketKey::new(Timeframe::M1, date(2025, 1, 2));
        assert_eq!(bucket.start_ms(), Some(1_735_776_000_000));
        assert_eq!(bucket.end_exclusive_ms(), Some(1_735_862_400_000));
    }

    #[test]
    fn discovery_reads_only_the_timeframe_depth() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("dukascopy").join("EURUSD");
        // Zero-based months: 2024/11 is December, 2025/00 is January.
        std::fs::create_dir_all(base.join("2024/11/31")).unwrap();
        std::fs::create_dir_all(base.join("2025/00/01")).unwrap();
        std::fs::create_dir_all(base.join("notes")).unwrap();

        let d1 = existing_buckets(&base, Timeframe::D1).unwrap();
        assert_eq!(d1.len(), 2);
        assert_eq!(d1[1].to_string(), "2025");

        let h1 = existing_buckets(&base, Timeframe::H1).unwrap();
        assert_eq!(h1.len(), 2);
        assert_eq!(h1[0].start(), date(2024, 12, 1));
        assert_eq!(h1[0].to_string(), "2024/11");

        let m1 = existing_buckets(&base, Timeframe::M1).unwrap();
        assert_eq!(m1.len(), 2);
        assert_eq!(m1[1].start(), date(2025, 1, 1));
        assert_eq!(m1[1].to_string(), "2025/00/01");
    }

    #[test]
    fn discovery_ignores_out_of_range_month_index() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("dukascopy").join("EURUSD");
        std::fs::create_dir_all(base.join("2025/11")).unwrap();
        std::fs::create_dir_all(base.join("2025/12")).unwrap();

        let h1 = existing_buckets(&base, Timeframe::H1).unwrap();
        assert_eq!(h1.len(), 1);
        assert_eq!(h1[0].start(), date(2025, 12, 1));
    }

    #[test]
    fn discovery_reports_missing_directory_with_path() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("dukascopy").join("NOPE");
        let err = existing_buckets(&missing, Timeframe::D1).unwrap_err();
        match err {
            ReaderError::Io { path, .. } => assert_eq!(path, missing),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn symbol_dir_appends_the_mirror_prefix() {
        assert_eq!(
            symbol_dir(Path::new("/data"), &sym("EURUSD")),
            Path::new("/data/dukascopy/EURUSD")
        );
    }

    #[test]
    fn require_dir_accepts_directories_only() {
        let tmp = TempDir::new().unwrap();
        require_dir(tmp.path()).unwrap();
        let file = tmp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        match require_dir(&file).unwrap_err() {
            ReaderError::Io { path, .. } => assert_eq!(path, file),
            other => panic!("unexpected error: {other}"),
        }
    }
}
