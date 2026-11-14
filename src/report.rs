//! Load report: what was requested, what was missing and what was cleaned.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::gaps::GapSummary;
use crate::layout::BucketKey;

/// One requested archive file that does not exist on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingFile {
    bucket: BucketKey,
    path: PathBuf,
}

impl MissingFile {
    pub(crate) fn new(bucket: BucketKey, path: PathBuf) -> Self {
        Self { bucket, path }
    }

    /// Calendar bucket the missing file covers.
    #[must_use]
    pub fn bucket(&self) -> BucketKey {
        self.bucket
    }

    /// Expected absolute path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for MissingFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.path.display(), self.bucket)
    }
}

/// Deterministic summary of one load.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    pub(crate) requested_files: usize,
    pub(crate) loaded_files: usize,
    pub(crate) missing_files: Vec<MissingFile>,
    pub(crate) rows_decoded: usize,
    pub(crate) rows_trimmed: usize,
    pub(crate) rows_invalid: usize,
    pub(crate) rows_out_of_bucket: usize,
    pub(crate) duplicates_removed: usize,
    pub(crate) gaps: GapSummary,
    pub(crate) first_bar_ms: Option<i64>,
    pub(crate) last_bar_ms: Option<i64>,
}

impl LoadReport {
    /// Number of archive files implied by the request.
    #[must_use]
    pub fn requested_files(&self) -> usize {
        self.requested_files
    }

    /// Number of archive files that existed and decoded successfully.
    #[must_use]
    pub fn loaded_files(&self) -> usize {
        self.loaded_files
    }

    /// Requested files that were absent.
    #[must_use]
    pub fn missing_files(&self) -> &[MissingFile] {
        &self.missing_files
    }

    /// Valid rows decoded before trimming and de-duplication.
    #[must_use]
    pub fn rows_decoded(&self) -> usize {
        self.rows_decoded
    }

    /// Rows removed because they fell outside the configured date range.
    #[must_use]
    pub fn rows_trimmed(&self) -> usize {
        self.rows_trimmed
    }

    /// Rows removed because OHLC was invalid.
    #[must_use]
    pub fn rows_invalid(&self) -> usize {
        self.rows_invalid
    }

    /// Rows removed because their timestamp fell outside their bucket.
    #[must_use]
    pub fn rows_out_of_bucket(&self) -> usize {
        self.rows_out_of_bucket
    }

    /// Rows removed as duplicate timestamps (first occurrence kept).
    #[must_use]
    pub fn duplicates_removed(&self) -> usize {
        self.duplicates_removed
    }

    /// Gap statistics for the returned series.
    #[must_use]
    pub fn gaps(&self) -> &GapSummary {
        &self.gaps
    }

    /// First returned bar open time (Unix milliseconds).
    #[must_use]
    pub fn first_bar_ms(&self) -> Option<i64> {
        self.first_bar_ms
    }

    /// Last returned bar open time (Unix milliseconds).
    #[must_use]
    pub fn last_bar_ms(&self) -> Option<i64> {
        self.last_bar_ms
    }

    /// Number of rows handed to the caller.
    #[must_use]
    pub fn rows_returned(&self) -> usize {
        self.rows_decoded
            .saturating_sub(self.rows_trimmed)
            .saturating_sub(self.duplicates_removed)
    }

    /// Whether every requested file loaded and no row-level anomaly was found.
    ///
    /// Date trimming is an explicit request, not an anomaly, so it does not
    /// affect completeness.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing_files.is_empty()
            && self.rows_invalid == 0
            && self.rows_out_of_bucket == 0
            && self.duplicates_removed == 0
            && self.gaps.segments() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gaps::GapSummary;
    use crate::types::Timeframe;
    use chrono::NaiveDate;

    fn missing_file() -> MissingFile {
        let bucket = BucketKey::new(Timeframe::D1, NaiveDate::from_ymd_opt(2025, 6, 1).unwrap());
        MissingFile::new(
            bucket,
            PathBuf::from("/data/dukascopy/EURUSD/2025/BID_candles_day_1.bi5"),
        )
    }

    #[test]
    fn default_report_is_empty_but_complete() {
        let report = LoadReport::default();
        assert_eq!(report.rows_returned(), 0);
        assert!(report.is_complete());
    }

    #[test]
    fn missing_files_break_completeness() {
        let mut report = LoadReport {
            requested_files: 1,
            ..LoadReport::default()
        };
        report.missing_files.push(missing_file());
        assert!(!report.is_complete());
        assert_eq!(
            report.missing_files()[0].to_string(),
            "/data/dukascopy/EURUSD/2025/BID_candles_day_1.bi5 (2025)"
        );
    }

    #[test]
    fn row_anomalies_break_completeness() {
        for mutate in [
            |r: &mut LoadReport| r.rows_invalid = 1,
            |r: &mut LoadReport| r.rows_out_of_bucket = 1,
            |r: &mut LoadReport| r.duplicates_removed = 1,
            |r: &mut LoadReport| r.gaps = GapSummary::scan(&[0, 120_000], 60_000, 1),
        ] {
            let mut report = LoadReport::default();
            mutate(&mut report);
            assert!(!report.is_complete());
        }
    }

    #[test]
    fn rows_returned_subtracts_trimmed_and_duplicates() {
        let report = LoadReport {
            rows_decoded: 100,
            rows_trimmed: 10,
            duplicates_removed: 5,
            ..LoadReport::default()
        };
        assert_eq!(report.rows_returned(), 85);
    }
}
