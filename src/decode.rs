//! Decode a Dukascopy `.bi5` file directly into columns.
//!
//! A `.bi5` file is an LZMA-alone stream of fixed 24-byte big-endian records:
//!
//! ```text
//! u32 time_seconds | u32 open | u32 close | u32 low | u32 high | u32 volume
//! ```
//!
//! Prices are fixed-point with five decimals. The field order (`O,C,L,H`) was
//! verified against real files: it satisfies the high/low envelope for every
//! bar, whereas `O,H,L,C` does not.
//!
//! Rows are pushed straight into the six output vectors; no intermediate
//! record struct is allocated.

use std::io::Read;
use std::path::Path;

use crate::error::{ReaderError, Result};
use crate::layout::BucketKey;

/// Size of one encoded candle record.
pub(crate) const RECORD_SIZE: usize = 24;
/// Dukascopy prices are stored as integers scaled by 100_000.
const PRICE_SCALE: f64 = 100_000.0;

/// One decoded file, already split into columns.
#[derive(Debug, Default)]
pub(crate) struct DecodedFile {
    pub time_ms: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<u32>,
    /// Rows dropped because OHLC was invalid.
    pub rows_invalid: usize,
    /// Rows dropped because their timestamp fell outside the bucket.
    pub rows_out_of_bucket: usize,
}

/// Open, decompress and decode one `.bi5` file.
pub(crate) fn decode_file(path: &Path, bucket: BucketKey) -> Result<DecodedFile> {
    let file = std::fs::File::open(path).map_err(|e| ReaderError::io(path, e))?;
    let reader = std::io::BufReader::new(file);
    let mut lzma = lzma_rust2::LzmaReader::new_mem_limit(reader, u32::MAX, None)
        .map_err(|e| ReaderError::corrupt(path, format!("lzma header: {e}")))?;
    let mut raw = Vec::new();
    lzma.read_to_end(&mut raw)
        .map_err(|e| ReaderError::corrupt(path, format!("lzma: {e}")))?;
    decode_records(&raw, bucket, path)
}

fn decode_records(raw: &[u8], bucket: BucketKey, path: &Path) -> Result<DecodedFile> {
    let (records, remainder) = raw.as_chunks::<RECORD_SIZE>();
    if !remainder.is_empty() {
        return Err(ReaderError::corrupt(
            path,
            format!(
                "decompressed size {} is not a multiple of {RECORD_SIZE}",
                raw.len()
            ),
        ));
    }
    let bucket_start_ms = bucket.start_ms().ok_or_else(|| {
        ReaderError::Integrity(format!("bucket {} has no UTC midnight", bucket.start()))
    })?;
    let bucket_end_ms = bucket.end_exclusive_ms();

    let capacity = raw.len() / RECORD_SIZE;
    let mut out = DecodedFile {
        time_ms: Vec::with_capacity(capacity),
        open: Vec::with_capacity(capacity),
        high: Vec::with_capacity(capacity),
        low: Vec::with_capacity(capacity),
        close: Vec::with_capacity(capacity),
        volume: Vec::with_capacity(capacity),
        ..DecodedFile::default()
    };

    for rec in records {
        let time_ms = bucket_start_ms + i64::from(be_u32(rec, 0)) * 1_000;
        if let Some(end) = bucket_end_ms
            && !(bucket_start_ms..end).contains(&time_ms)
        {
            out.rows_out_of_bucket += 1;
            continue;
        }

        let open = f64::from(be_u32(rec, 4)) / PRICE_SCALE;
        let close = f64::from(be_u32(rec, 8)) / PRICE_SCALE;
        let low = f64::from(be_u32(rec, 12)) / PRICE_SCALE;
        let high = f64::from(be_u32(rec, 16)) / PRICE_SCALE;
        let volume = be_u32(rec, 20);

        if !is_valid_ohlc(open, high, low, close) {
            out.rows_invalid += 1;
            continue;
        }

        out.time_ms.push(time_ms);
        out.open.push(open);
        out.high.push(high);
        out.low.push(low);
        out.close.push(close);
        out.volume.push(volume);
    }
    Ok(out)
}

/// Read a big-endian `u32` at one of the fixed record offsets.
fn be_u32(rec: &[u8; RECORD_SIZE], offset: usize) -> u32 {
    // INVARIANT: callers only use offsets 0, 4, 8, 12, 16 and 20.
    u32::from_be_bytes([
        rec[offset],
        rec[offset + 1],
        rec[offset + 2],
        rec[offset + 3],
    ])
}

/// Reject records whose prices are zero or violate the OHLC envelope.
fn is_valid_ohlc(open: f64, high: f64, low: f64, close: f64) -> bool {
    open > 0.0
        && high > 0.0
        && low > 0.0
        && close > 0.0
        && high >= open
        && high >= close
        && high >= low
        && low <= open
        && low <= close
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn bucket(tf: crate::types::Timeframe, y: i32, m: u32, d: u32) -> BucketKey {
        BucketKey::new(tf, NaiveDate::from_ymd_opt(y, m, d).unwrap())
    }

    fn record(time: u32, open: u32, close: u32, low: u32, high: u32, volume: u32) -> [u8; 24] {
        let mut rec = [0u8; 24];
        rec[0..4].copy_from_slice(&time.to_be_bytes());
        rec[4..8].copy_from_slice(&open.to_be_bytes());
        rec[8..12].copy_from_slice(&close.to_be_bytes());
        rec[12..16].copy_from_slice(&low.to_be_bytes());
        rec[16..20].copy_from_slice(&high.to_be_bytes());
        rec[20..24].copy_from_slice(&volume.to_be_bytes());
        rec
    }

    fn decode(raw: &[u8], tf: crate::types::Timeframe) -> Result<DecodedFile> {
        let b = bucket(tf, 2025, 1, 1);
        decode_records(raw, b, Path::new("test.bi5"))
    }

    #[test]
    fn decodes_record_into_columns() {
        let raw = record(0, 100_000, 100_500, 99_000, 101_000, 5_000);
        let out = decode(&raw, crate::types::Timeframe::D1).unwrap();
        assert_eq!(out.time_ms.len(), 1);
        assert_eq!(out.time_ms[0], 1_735_689_600_000);
        assert!((out.open[0] - 1.0).abs() < 1e-9);
        assert!((out.high[0] - 1.01).abs() < 1e-9);
        assert!((out.low[0] - 0.99).abs() < 1e-9);
        assert!((out.close[0] - 1.005).abs() < 1e-9);
        assert_eq!(out.volume, [5_000]);
    }

    #[test]
    fn decodes_multiple_records_in_order() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&record(0, 100_000, 100_500, 99_000, 101_000, 5_000));
        raw.extend_from_slice(&record(86_400, 101_000, 101_500, 100_000, 102_000, 6_000));
        let out = decode(&raw, crate::types::Timeframe::D1).unwrap();
        assert_eq!(out.time_ms.len(), 2);
        assert_eq!(out.time_ms[1] - out.time_ms[0], 86_400_000);
    }

    #[test]
    fn empty_payload_is_valid_and_empty() {
        let out = decode(&[], crate::types::Timeframe::D1).unwrap();
        assert!(out.time_ms.is_empty());
        assert_eq!(out.rows_invalid, 0);
    }

    #[test]
    fn invalid_ohlc_rows_are_counted_not_fatal() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&record(0, 100_000, 100_500, 99_000, 101_000, 5_000));
        // high < low
        raw.extend_from_slice(&record(86_400, 100_000, 100_500, 101_000, 99_000, 5_000));
        // zero price
        raw.extend_from_slice(&record(172_800, 0, 0, 0, 0, 5_000));
        let out = decode(&raw, crate::types::Timeframe::D1).unwrap();
        assert_eq!(out.time_ms.len(), 1);
        assert_eq!(out.rows_invalid, 2);
    }

    #[test]
    fn timestamps_outside_the_bucket_are_counted_not_fatal() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&record(0, 100_000, 100_500, 99_000, 101_000, 5_000));
        // 2025-01-01 00:00:00 belongs to the next year bucket.
        raw.extend_from_slice(&record(86_400, 100_000, 100_500, 99_000, 101_000, 5_000));
        let out = decode(&raw, crate::types::Timeframe::M1).unwrap();
        assert_eq!(out.time_ms.len(), 1);
        assert_eq!(out.rows_out_of_bucket, 1);
    }

    #[test]
    fn truncated_record_is_a_corrupt_file() {
        let err = decode(&[0u8; 25], crate::types::Timeframe::D1).unwrap_err();
        match err {
            ReaderError::CorruptFile { detail, .. } => {
                assert!(detail.contains("not a multiple"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn garbage_lzma_is_a_corrupt_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("BID_candles_day_1.bi5");
        std::fs::write(&path, b"definitely not lzma").unwrap();
        let err = decode_file(&path, bucket(crate::types::Timeframe::D1, 2025, 1, 1)).unwrap_err();
        assert!(matches!(err, ReaderError::CorruptFile { .. }));
    }

    #[test]
    fn ohlc_envelope_rules() {
        assert!(is_valid_ohlc(10.0, 10.0, 10.0, 10.0));
        assert!(is_valid_ohlc(10.0, 12.0, 9.0, 11.0));
        assert!(!is_valid_ohlc(10.0, 9.0, 12.0, 11.0));
        assert!(!is_valid_ohlc(10.0, 12.0, 9.0, 0.0));
    }
}
