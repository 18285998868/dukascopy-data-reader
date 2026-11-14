//! Column-oriented candle output and the internal append buffer.
//!
//! Prices are `f64` (five-decimal fixed point decoded exactly) and volumes are
//! the raw unsigned integers stored by Dukascopy. Time is Unix milliseconds at
//! bar open, UTC.

use crate::decode::DecodedFile;
use crate::error::{ReaderError, Result};

/// Owned, equal-length column vectors.
///
/// Unlike a tuple, this named type keeps `into_parts` readable and avoids
/// `clippy::type_complexity`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColumnParts {
    /// Bar open times in Unix milliseconds (UTC).
    pub time: Vec<i64>,
    /// Open prices.
    pub open: Vec<f64>,
    /// High prices.
    pub high: Vec<f64>,
    /// Low prices.
    pub low: Vec<f64>,
    /// Close prices.
    pub close: Vec<f64>,
    /// Raw Dukascopy volume units (not normalised).
    pub volume: Vec<u32>,
}

/// Column-oriented OHLCV output.
#[derive(Debug, Clone, PartialEq)]
pub struct KlineColumns {
    parts: ColumnParts,
}

impl KlineColumns {
    /// Build columns from owned parts, validating equal lengths.
    ///
    /// # Errors
    ///
    /// Returns [`ReaderError::Integrity`] when the six vectors differ in
    /// length.
    pub fn from_parts(parts: ColumnParts) -> Result<Self> {
        let len = parts.time.len();
        for (name, column_len) in [
            ("open", parts.open.len()),
            ("high", parts.high.len()),
            ("low", parts.low.len()),
            ("close", parts.close.len()),
            ("volume", parts.volume.len()),
        ] {
            if column_len != len {
                return Err(ReaderError::Integrity(format!(
                    "column {name} has length {column_len}, expected {len}"
                )));
            }
        }
        Ok(Self { parts })
    }

    /// Bar open times in Unix milliseconds (UTC).
    #[must_use]
    pub fn time(&self) -> &[i64] {
        &self.parts.time
    }

    /// Open prices.
    #[must_use]
    pub fn open(&self) -> &[f64] {
        &self.parts.open
    }

    /// High prices.
    #[must_use]
    pub fn high(&self) -> &[f64] {
        &self.parts.high
    }

    /// Low prices.
    #[must_use]
    pub fn low(&self) -> &[f64] {
        &self.parts.low
    }

    /// Close prices.
    #[must_use]
    pub fn close(&self) -> &[f64] {
        &self.parts.close
    }

    /// Raw Dukascopy volume units (not normalised).
    #[must_use]
    pub fn volume(&self) -> &[u32] {
        &self.parts.volume
    }

    /// Number of bars.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parts.time.len()
    }

    /// Whether there are no bars.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.time.is_empty()
    }

    /// Consume the columns and return the owned vectors.
    #[must_use]
    pub fn into_parts(self) -> ColumnParts {
        self.parts
    }
}

/// Accumulates per-file columns in request order before final cleanup.
#[derive(Debug, Default)]
pub(crate) struct ColumnBuffer {
    parts: ColumnParts,
}

impl ColumnBuffer {
    /// Pre-allocate for an estimated number of bars.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            parts: ColumnParts {
                time: Vec::with_capacity(capacity),
                open: Vec::with_capacity(capacity),
                high: Vec::with_capacity(capacity),
                low: Vec::with_capacity(capacity),
                close: Vec::with_capacity(capacity),
                volume: Vec::with_capacity(capacity),
            },
        }
    }

    /// Append one decoded file (no copy beyond vector growth).
    pub(crate) fn append(&mut self, mut file: DecodedFile) {
        self.parts.time.append(&mut file.time_ms);
        self.parts.open.append(&mut file.open);
        self.parts.high.append(&mut file.high);
        self.parts.low.append(&mut file.low);
        self.parts.close.append(&mut file.close);
        self.parts.volume.append(&mut file.volume);
    }

    pub(crate) fn len(&self) -> usize {
        self.parts.time.len()
    }

    pub(crate) fn time(&self) -> &[i64] {
        &self.parts.time
    }

    /// Drop bars outside `[start_ms, end_ms_exclusive)`, returning how many
    /// were removed.
    pub(crate) fn trim(&mut self, start_ms: Option<i64>, end_ms_exclusive: Option<i64>) -> usize {
        if start_ms.is_none() && end_ms_exclusive.is_none() {
            return 0;
        }
        let len = self.parts.time.len();
        let mut write = 0usize;
        for read in 0..len {
            let time = self.parts.time[read];
            let keep = start_ms.is_none_or(|start| time >= start)
                && end_ms_exclusive.is_none_or(|end| time < end);
            if keep {
                if read != write {
                    self.copy_row(read, write);
                }
                write += 1;
            }
        }
        let removed = len - write;
        self.truncate(write);
        removed
    }

    /// Sort all columns by time only when the series is not already ascending.
    ///
    /// Files are decoded in bucket order, so the sorted path is the normal one
    /// and the permutation fallback only runs on out-of-order archives.
    pub(crate) fn sort_if_needed(&mut self) {
        if self.parts.time.windows(2).all(|w| w[0] <= w[1]) {
            return;
        }
        let mut perm: Vec<usize> = (0..self.parts.time.len()).collect();
        perm.sort_by_key(|&index| self.parts.time[index]);
        let time = permute(&self.parts.time, &perm);
        let open = permute(&self.parts.open, &perm);
        let high = permute(&self.parts.high, &perm);
        let low = permute(&self.parts.low, &perm);
        let close = permute(&self.parts.close, &perm);
        let volume = permute(&self.parts.volume, &perm);
        self.parts = ColumnParts {
            time,
            open,
            high,
            low,
            close,
            volume,
        };
    }

    /// Remove duplicate timestamps, keeping the first occurrence, and return
    /// how many rows were removed.
    ///
    /// The series must already be sorted by time.
    pub(crate) fn dedup(&mut self) -> usize {
        let len = self.parts.time.len();
        if len < 2 {
            return 0;
        }
        let mut write = 1usize;
        for read in 1..len {
            if self.parts.time[read] == self.parts.time[write - 1] {
                continue;
            }
            if read != write {
                self.copy_row(read, write);
            }
            write += 1;
        }
        let removed = len - write;
        self.truncate(write);
        removed
    }

    pub(crate) fn finish(self) -> KlineColumns {
        KlineColumns { parts: self.parts }
    }

    /// Copy one complete row to an earlier index.
    ///
    /// INVARIANT: `to <= from < len`; every column has the same length.
    fn copy_row(&mut self, from: usize, to: usize) {
        self.parts.time[to] = self.parts.time[from];
        self.parts.open[to] = self.parts.open[from];
        self.parts.high[to] = self.parts.high[from];
        self.parts.low[to] = self.parts.low[from];
        self.parts.close[to] = self.parts.close[from];
        self.parts.volume[to] = self.parts.volume[from];
    }

    fn truncate(&mut self, len: usize) {
        self.parts.time.truncate(len);
        self.parts.open.truncate(len);
        self.parts.high.truncate(len);
        self.parts.low.truncate(len);
        self.parts.close.truncate(len);
        self.parts.volume.truncate(len);
    }
}

fn permute<T: Copy>(values: &[T], perm: &[usize]) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len());
    out.extend(perm.iter().map(|&index| values[index]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(time: &[i64], price: f64, volume: u32) -> DecodedFile {
        DecodedFile {
            time_ms: time.to_vec(),
            open: vec![price; time.len()],
            high: vec![price + 1.0; time.len()],
            low: vec![price - 1.0; time.len()],
            close: vec![price; time.len()],
            volume: vec![volume; time.len()],
            ..DecodedFile::default()
        }
    }

    fn buffer(files: Vec<DecodedFile>) -> ColumnBuffer {
        let mut buffer = ColumnBuffer::default();
        for file in files {
            buffer.append(file);
        }
        buffer
    }

    #[test]
    fn parts_must_have_equal_lengths() {
        let parts = ColumnParts {
            time: vec![1, 2],
            open: vec![1.0],
            ..ColumnParts::default()
        };
        assert!(KlineColumns::from_parts(parts).is_err());
    }

    #[test]
    fn roundtrips_through_parts() {
        let cols = KlineColumns::from_parts(ColumnParts {
            time: vec![1, 2],
            open: vec![1.0, 2.0],
            high: vec![3.0, 4.0],
            low: vec![0.5, 1.5],
            close: vec![2.0, 3.0],
            volume: vec![10, 20],
        })
        .unwrap();
        assert_eq!(cols.len(), 2);
        assert!(!cols.is_empty());
        let parts = cols.into_parts();
        assert_eq!(parts.time, [1, 2]);
        assert_eq!(parts.volume, [10, 20]);
    }

    #[test]
    fn append_keeps_columns_aligned() {
        let mut buffer = buffer(vec![file(&[30, 10], 2.0, 7), file(&[20], 3.0, 8)]);
        assert_eq!(buffer.len(), 3);
        buffer.sort_if_needed();
        buffer.dedup();
        let cols = buffer.finish();
        assert_eq!(cols.time(), [10, 20, 30]);
        assert_eq!(cols.close(), [2.0, 3.0, 2.0]);
        assert_eq!(cols.volume(), [7, 8, 7]);
    }

    #[test]
    fn sort_reorders_every_column() {
        let mut buffer = buffer(vec![file(&[30, 10, 20], 2.0, 7)]);
        buffer.sort_if_needed();
        let cols = buffer.finish();
        assert_eq!(cols.time(), [10, 20, 30]);
        assert_eq!(cols.high(), [3.0, 3.0, 3.0]);
    }

    #[test]
    fn sort_is_a_noop_when_already_sorted() {
        let mut buffer = buffer(vec![file(&[10, 20, 30], 2.0, 7)]);
        buffer.sort_if_needed();
        assert_eq!(buffer.time(), [10, 20, 30]);
    }

    #[test]
    fn dedup_keeps_first_row_and_counts() {
        let mut buffer = buffer(vec![file(&[10, 10, 20], 2.0, 7)]);
        assert_eq!(buffer.dedup(), 1);
        let cols = buffer.finish();
        assert_eq!(cols.time(), [10, 20]);
        assert_eq!(cols.volume(), [7, 7]);
    }

    #[test]
    fn trim_bounds_are_start_inclusive_end_exclusive() {
        let mut buffer = buffer(vec![file(&[0, 10, 20, 30], 2.0, 7)]);
        assert_eq!(buffer.trim(Some(10), Some(30)), 2);
        let cols = buffer.finish();
        assert_eq!(cols.time(), [10, 20]);
    }

    #[test]
    fn trim_without_bounds_is_a_noop() {
        let mut buffer = buffer(vec![file(&[10, 20], 2.0, 7)]);
        assert_eq!(buffer.trim(None, None), 0);
        assert_eq!(buffer.len(), 2);
    }
}
