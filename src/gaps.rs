//! Gap detection on a sorted, de-duplicated time column.
//!
//! Gaps are reported, never filled: the series handed to the caller stays
//! exactly as stored in the archive.

/// One stretch between two adjacent bars whose spacing is larger than the
/// timeframe interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    prev_time_ms: i64,
    next_time_ms: i64,
    missing_bars: i64,
    misaligned: bool,
}

impl Gap {
    /// Open time of the bar before the gap (Unix milliseconds).
    #[must_use]
    pub fn prev_time_ms(&self) -> i64 {
        self.prev_time_ms
    }

    /// Open time of the bar after the gap (Unix milliseconds).
    #[must_use]
    pub fn next_time_ms(&self) -> i64 {
        self.next_time_ms
    }

    /// Number of expected bars missing between the two neighbours.
    #[must_use]
    pub fn missing_bars(&self) -> i64 {
        self.missing_bars
    }

    /// Whether the spacing is not a whole multiple of the interval.
    #[must_use]
    pub fn is_misaligned(&self) -> bool {
        self.misaligned
    }
}

/// Aggregate gap statistics plus a bounded sample of gap segments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GapSummary {
    segments: usize,
    missing_bars: i64,
    misaligned_segments: usize,
    samples: Vec<Gap>,
}

impl GapSummary {
    /// Number of gap segments.
    #[must_use]
    pub fn segments(&self) -> usize {
        self.segments
    }

    /// Total bars missing across all segments.
    #[must_use]
    pub fn missing_bars(&self) -> i64 {
        self.missing_bars
    }

    /// Segments whose spacing is not a whole multiple of the interval.
    #[must_use]
    pub fn misaligned_segments(&self) -> usize {
        self.misaligned_segments
    }

    /// First `max_samples` gap segments.
    #[must_use]
    pub fn samples(&self) -> &[Gap] {
        &self.samples
    }

    /// Scan an ascending time column for missing bars.
    pub(crate) fn scan(time_ms: &[i64], interval_ms: i64, max_samples: usize) -> Self {
        let mut summary = Self::default();
        if interval_ms <= 0 || time_ms.len() < 2 {
            return summary;
        }
        for window in time_ms.windows(2) {
            let diff = window[1].saturating_sub(window[0]);
            if diff == interval_ms {
                continue;
            }
            let missing_bars = (diff / interval_ms - 1).max(0);
            let misaligned = diff.rem_euclid(interval_ms) != 0;
            summary.segments += 1;
            summary.missing_bars = summary.missing_bars.saturating_add(missing_bars);
            if misaligned {
                summary.misaligned_segments += 1;
            }
            if summary.samples.len() < max_samples {
                summary.samples.push(Gap {
                    prev_time_ms: window[0],
                    next_time_ms: window[1],
                    missing_bars,
                    misaligned,
                });
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_series_has_no_gaps() {
        let summary = GapSummary::scan(&[0, 60_000, 120_000], 60_000, 8);
        assert_eq!(summary.segments(), 0);
        assert_eq!(summary.missing_bars(), 0);
        assert!(summary.samples().is_empty());
    }

    #[test]
    fn missing_bars_are_counted() {
        let summary = GapSummary::scan(&[0, 60_000, 300_000], 60_000, 8);
        assert_eq!(summary.segments(), 1);
        assert_eq!(summary.missing_bars(), 3);
        assert_eq!(summary.misaligned_segments(), 0);
        assert_eq!(summary.samples()[0].prev_time_ms(), 60_000);
        assert_eq!(summary.samples()[0].next_time_ms(), 300_000);
        assert_eq!(summary.samples()[0].missing_bars(), 3);
        assert!(!summary.samples()[0].is_misaligned());
    }

    #[test]
    fn misaligned_spacing_is_flagged() {
        let summary = GapSummary::scan(&[0, 90_000], 60_000, 8);
        assert_eq!(summary.segments(), 1);
        assert_eq!(summary.missing_bars(), 0);
        assert_eq!(summary.misaligned_segments(), 1);
        assert!(summary.samples()[0].is_misaligned());
    }

    #[test]
    fn samples_are_capped_but_totals_are_not() {
        let times: Vec<i64> = (0..20).map(|i| i * 120_000).collect();
        let summary = GapSummary::scan(&times, 60_000, 3);
        assert_eq!(summary.segments(), 19);
        assert_eq!(summary.missing_bars(), 19);
        assert_eq!(summary.samples().len(), 3);
    }

    #[test]
    fn short_or_degenerate_input_is_empty() {
        assert_eq!(GapSummary::scan(&[], 60_000, 8).segments(), 0);
        assert_eq!(GapSummary::scan(&[0], 60_000, 8).segments(), 0);
        assert_eq!(GapSummary::scan(&[0, 60_000], 0, 8).segments(), 0);
    }
}
