use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Datelike;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use rayon::prelude::*;

use crate::app::Record;
use crate::error::ReaderError;
use crate::error::Result;
use crate::types::{Symbol, Timeframe};

#[derive(Debug, Clone, Copy, PartialEq)]
enum PeriodKind {
    Year,
    Month,
    Day,
}

/// Map a requested timeframe to the Dukascopy candles filename part.
/// `Timeframe` 已按源校验，此处未知映射仍返回 `Err`（不在 release 静默回落），
/// 避免白名单先扩而映射未加时错拿日线数据。
fn expected_tf_part(timeframe: &Timeframe) -> Result<&'static str> {
    match timeframe.as_str() {
        "M1" | "1m" => Ok("candles_min_1"),
        "H1" | "1h" => Ok("candles_hour_1"),
        "D1" | "1d" => Ok("candles_day_1"),
        other => Err(ReaderError::Config(format!(
            "unsupported dukascopy timeframe: {other}"
        ))),
    }
}

fn period_kind_for_timeframe(timeframe: &Timeframe) -> Result<PeriodKind> {
    Ok(match expected_tf_part(timeframe)? {
        "candles_min_1" => PeriodKind::Day,
        "candles_hour_1" => PeriodKind::Month,
        _ => PeriodKind::Year,
    })
}

/// Classify an official-mirror filename `{PRICE}_{candles_*}.bi5`.
fn kind_from_filename(name: &str) -> Option<PeriodKind> {
    let stem = name.strip_suffix(".bi5")?;
    let (_, tf_part) = stem.split_once('_')?;
    match tf_part {
        "candles_min_1" => Some(PeriodKind::Day),
        "candles_hour_1" => Some(PeriodKind::Month),
        "candles_day_1" => Some(PeriodKind::Year),
        _ => None,
    }
}

/// Parse an official-mirror .bi5 path below `{dir}/dukascopy/{symbol}/`.
///
/// Expected relative shape: `{YYYY}/[{MM}/[{DD}/]]{PRICE}_{candles_*}.bi5`.
/// Files whose candles part does not match `timeframe` are skipped (`Ok(None)`);
/// an unmappable timeframe is a hard `Err` (never silently fall back).
fn parse_bi5_path(
    base: &Path,
    path: &Path,
    timeframe: &Timeframe,
) -> Result<Option<(PeriodKind, NaiveDate)>> {
    if path.extension().and_then(|s| s.to_str()) != Some("bi5") {
        return Ok(None);
    }
    let Some(rel) = path.strip_prefix(base).ok() else {
        return Ok(None);
    };
    let Some(name) = rel.file_name().and_then(|s| s.to_str()) else {
        return Ok(None);
    };
    let Some(kind) = kind_from_filename(name) else {
        return Ok(None);
    };
    if kind != period_kind_for_timeframe(timeframe)? {
        return Ok(None);
    }
    let Some(parent) = rel.parent() else {
        return Ok(None);
    };
    let comps: Vec<&str> = parent
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let date = match kind {
        PeriodKind::Year => {
            let [y] = comps.as_slice() else {
                return Ok(None);
            };
            let Ok(y) = y.parse::<i32>() else {
                return Ok(None);
            };
            let Some(d) = NaiveDate::from_ymd_opt(y, 1, 1) else {
                return Ok(None);
            };
            d
        }
        PeriodKind::Month => {
            let [y, m] = comps.as_slice() else {
                return Ok(None);
            };
            let (Ok(y), Ok(m)) = (y.parse::<i32>(), m.parse::<u32>()) else {
                return Ok(None);
            };
            let Some(d) = NaiveDate::from_ymd_opt(y, m, 1) else {
                return Ok(None);
            };
            d
        }
        PeriodKind::Day => {
            let [y, m, d] = comps.as_slice() else {
                return Ok(None);
            };
            let (Ok(y), Ok(m), Ok(day)) = (y.parse::<i32>(), m.parse::<u32>(), d.parse::<u32>())
            else {
                return Ok(None);
            };
            let Some(date) = NaiveDate::from_ymd_opt(y, m, day) else {
                return Ok(None);
            };
            date
        }
    };
    Ok(Some((kind, date)))
}

fn period_end(period_start: NaiveDate, kind: PeriodKind) -> Result<NaiveDate> {
    let next = match kind {
        PeriodKind::Year => period_start
            .year()
            .checked_add(1)
            .and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1)),
        PeriodKind::Month => {
            if period_start.month() == 12 {
                period_start
                    .year()
                    .checked_add(1)
                    .and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1))
            } else {
                NaiveDate::from_ymd_opt(period_start.year(), period_start.month() + 1, 1)
            }
        }
        PeriodKind::Day => period_start.succ_opt(),
    };
    next.ok_or_else(|| {
        ReaderError::Parse(format!(
            "period end out of range for {period_start:?} ({kind:?})"
        ))
    })
}

fn should_skip_file(
    period_start: NaiveDate,
    kind: PeriodKind,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Result<bool> {
    let p_end = period_end(period_start, kind)?;

    if let Some(sd) = start_date
        && p_end <= sd
    {
        return Ok(true);
    }
    if let Some(ed) = end_date
        && period_start > ed
    {
        return Ok(true);
    }
    Ok(false)
}

fn decompress_bi5(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    lzma_rs::lzma_decompress(&mut reader, &mut buf)
        .map_err(|e| ReaderError::Parse(format!("lzma: {e}")))?;
    Ok(buf)
}

/// 解析 bi5 记录：返回值含跳过的坏行数（OHLC 口径与 binance 统一，
/// `super::is_valid_ohlc`），调用方汇总 warn。
fn parse_bi5_records(bytes: &[u8], period_start: NaiveDateTime) -> Result<(Vec<Record>, usize)> {
    const REC_SIZE: usize = 24;
    let num_records = bytes.len() / REC_SIZE;
    if num_records == 0 {
        return Ok((Vec::new(), 0));
    }
    if !bytes.len().is_multiple_of(REC_SIZE) {
        tracing::warn!("bi5 data size {} not multiple of 24", bytes.len());
    }

    let mut records = Vec::with_capacity(num_records);
    let mut skipped = 0usize;
    for i in 0..num_records {
        let offset = i * REC_SIZE;
        let rec: &[u8; REC_SIZE] = &bytes[offset..offset + REC_SIZE]
            .try_into()
            .map_err(|_| ReaderError::Parse("invalid bi5 record bounds".into()))?;

        let read_be_u32 = |start: usize| -> Result<u32> {
            rec[start..start + 4]
                .try_into()
                .map(u32::from_be_bytes)
                .map_err(|_| ReaderError::Parse("invalid bi5 field".into()))
        };

        let time_secs = i64::from(read_be_u32(0)?);
        // 字段序经实数据验证为 O,C,L,H（非 O,H,L,C）：365/365 满足 high/low 包络，
        // 且 |open[i+1]-close[i]| 均值 0.0002（反设为 0.0047）。旧代码 high/close 错位。
        let open = f64::from(read_be_u32(4)?) / 100_000.0;
        let close = f64::from(read_be_u32(8)?) / 100_000.0;
        let low = f64::from(read_be_u32(12)?) / 100_000.0;
        let high = f64::from(read_be_u32(16)?) / 100_000.0;
        let volume = f64::from(read_be_u32(20)?);
        if !super::is_valid_ohlc(open, high, low, close, volume) {
            skipped += 1;
            continue;
        }

        let ts = period_start + chrono::TimeDelta::seconds(time_secs);
        records.push(Record {
            open_time: ts.and_utc().timestamp_millis(),
            open,
            high,
            low,
            close,
            volume,
        });
    }
    Ok((records, skipped))
}

fn load_bi5_data(path: &Path, period_start: NaiveDate) -> Result<(Vec<Record>, usize)> {
    let bytes = decompress_bi5(path)?;
    let dt = period_start
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ReaderError::Parse("invalid period_start date".into()))?;
    parse_bi5_records(&bytes, dt)
}

#[cfg(test)]
fn load_single_bi5(path: &Path) -> Result<Vec<Record>> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ReaderError::Parse(format!("invalid path: {}", path.display())))?;
    let kind = kind_from_filename(name)
        .ok_or_else(|| ReaderError::Parse(format!("invalid bi5 filename: {}", path.display())))?;
    // Ancestor dirs above the file: [DD|MM|YYYY, ..., symbol, dukascopy, ...].
    let ancestors: Vec<&str> = path
        .ancestors()
        .skip(1)
        .filter_map(|a| a.file_name())
        .filter_map(|s| s.to_str())
        .collect();
    let period_start = match kind {
        PeriodKind::Year => {
            let y: i32 = ancestors
                .first()
                .ok_or_else(|| ReaderError::Parse("missing year dir".into()))?
                .parse()
                .map_err(|_| ReaderError::Parse("invalid year dir".into()))?;
            NaiveDate::from_ymd_opt(y, 1, 1)
                .ok_or_else(|| ReaderError::Parse("invalid year dir".into()))?
        }
        PeriodKind::Month => {
            let (m, y) = match ancestors.as_slice() {
                [m, y, ..] => (*m, *y),
                _ => return Err(ReaderError::Parse("missing month dirs".into())),
            };
            NaiveDate::from_ymd_opt(
                y.parse()
                    .map_err(|_| ReaderError::Parse("invalid year dir".into()))?,
                m.parse()
                    .map_err(|_| ReaderError::Parse("invalid month dir".into()))?,
                1,
            )
            .ok_or_else(|| ReaderError::Parse("invalid month dirs".into()))?
        }
        PeriodKind::Day => {
            let (d, m, y) = match ancestors.as_slice() {
                [d, m, y, ..] => (*d, *m, *y),
                _ => return Err(ReaderError::Parse("missing day dirs".into())),
            };
            NaiveDate::from_ymd_opt(
                y.parse()
                    .map_err(|_| ReaderError::Parse("invalid year dir".into()))?,
                m.parse()
                    .map_err(|_| ReaderError::Parse("invalid month dir".into()))?,
                d.parse()
                    .map_err(|_| ReaderError::Parse("invalid day dir".into()))?,
            )
            .ok_or_else(|| ReaderError::Parse("invalid day dirs".into()))?
        }
    };
    load_bi5_data(path, period_start).map(|(records, _)| records)
}

#[tracing::instrument(skip_all, fields(symbol, timeframe))]
pub(crate) fn load_dukascopy_pipeline(
    dir: impl AsRef<Path>,
    symbol: &Symbol,
    timeframe: &Timeframe,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Result<Vec<Record>> {
    let dir = dir.as_ref();
    let symbol_dir: PathBuf = dir.join("dukascopy").join(symbol.as_str());
    if !symbol_dir.exists() {
        return Err(ReaderError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("directory not found: {}/dukascopy/{symbol}", dir.display()),
        )));
    }

    let t0 = Instant::now();

    let mut paths: Vec<PathBuf> = walkdir::WalkDir::new(&symbol_dir)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(e) => Some(e),
            Err(err) => {
                tracing::warn!("walkdir entry failed: {err}");
                None
            }
        })
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|path| {
            path.extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("bi5"))
                .unwrap_or(false)
        })
        .collect();

    paths.par_sort_unstable();

    /// 单文件结局：`Loaded` 好数据、`Skipped` 意图内跳过（周期不匹配/区间外）、
    /// `Failed` 硬失败（解压/解析失败），计入零容忍收口。
    enum FileOutcome {
        Loaded(Vec<Record>, usize),
        Skipped,
        Failed(String),
    }

    let outcomes: Vec<FileOutcome> = paths
        .par_iter()
        .map(|path| {
            let (kind, period_start) = match parse_bi5_path(&symbol_dir, path, timeframe) {
                Ok(Some(parsed)) => parsed,
                Ok(None) => return FileOutcome::Skipped,
                Err(e) => return FileOutcome::Failed(format!("{}: {e}", path.display())),
            };
            match should_skip_file(period_start, kind, start_date, end_date) {
                Ok(true) => return FileOutcome::Skipped,
                Ok(false) => {}
                Err(e) => return FileOutcome::Failed(format!("{}: {e}", path.display())),
            }
            match load_bi5_data(path, period_start) {
                Ok((records, skipped)) => FileOutcome::Loaded(records, skipped),
                Err(e) => FileOutcome::Failed(format!("{}: {e}", path.display())),
            }
        })
        .collect();

    let mut failed: Vec<String> = Vec::new();
    let mut skipped_rows_total = 0usize;
    let mut all_records: Vec<Record> = Vec::new();
    for outcome in outcomes {
        match outcome {
            FileOutcome::Loaded(records, skipped) => {
                skipped_rows_total += skipped;
                all_records.extend(records);
            }
            FileOutcome::Skipped => {}
            FileOutcome::Failed(detail) => failed.push(detail),
        }
    }
    super::fail_on_hard_errors("dukascopy", &failed)?;
    if skipped_rows_total > 0 {
        tracing::warn!(
            rows = skipped_rows_total,
            "loaded dukascopy with data loss: invalid OHLC rows skipped"
        );
    }

    tracing::info!(
        rows = all_records.len(),
        elapsed = ?t0.elapsed(),
        "loaded dukascopy"
    );
    Ok(all_records)
}

/// Reads Dukascopy .bi5 kline files for a symbol+timeframe.
///
/// Directory layout (new official mirror): `{dir}/dukascopy/{symbol}/`
/// File naming: `{YYYY}/[{MM}/[{DD}/]]{PRICE}_{candles_*}.bi5`
///   - D1 (daily): `{YYYY}/BID_candles_day_1.bi5`
///   - H1 (hourly): `{YYYY}/{MM}/BID_candles_hour_1.bi5`
///   - M1 (minute): `{YYYY}/{MM}/{DD}/BID_candles_min_1.bi5`
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sym(s: &str) -> Symbol {
        Symbol::parse(s).unwrap()
    }
    fn tf_d(s: &str) -> Timeframe {
        Timeframe::parse(s).unwrap()
    }
    fn pd(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    // — kind_from_filename / parse_bi5_path —

    fn parsed(base: &Path, rel: &str, tf: &str) -> Option<(PeriodKind, NaiveDate)> {
        parse_bi5_path(base, &base.join(rel), &tf_d(tf)).unwrap()
    }

    fn skipped(
        period_start: NaiveDate,
        kind: PeriodKind,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
    ) -> bool {
        should_skip_file(period_start, kind, start, end).unwrap()
    }

    #[test]
    fn parse_path_d1_year() {
        let base = Path::new("/data/dukascopy/EURUSD");
        let (kind, date) = parsed(base, "2025/BID_candles_day_1.bi5", "D1").expect("parsed");
        assert_eq!(kind, PeriodKind::Year);
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());
    }

    #[test]
    fn parse_path_h1_month() {
        let base = Path::new("/data/dukascopy/EURUSD");
        let (kind, date) = parsed(base, "2025/03/ASK_candles_hour_1.bi5", "H1").expect("parsed");
        assert_eq!(kind, PeriodKind::Month);
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 3, 1).unwrap());
    }

    #[test]
    fn parse_path_m1_day() {
        let base = Path::new("/data/dukascopy/EURUSD");
        let (kind, date) = parsed(base, "2025/03/01/BID_candles_min_1.bi5", "M1").expect("parsed");
        assert_eq!(kind, PeriodKind::Day);
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 3, 1).unwrap());
    }

    #[test]
    fn parse_path_timeframe_aliases() {
        let base = Path::new("/data/dukascopy/X");
        assert_eq!(
            parsed(base, "2025/BID_candles_day_1.bi5", "1d")
                .expect("parsed")
                .0,
            PeriodKind::Year
        );
        assert_eq!(
            parsed(base, "2025/03/BID_candles_hour_1.bi5", "1h")
                .expect("parsed")
                .0,
            PeriodKind::Month
        );
        assert_eq!(
            parsed(base, "2025/03/01/BID_candles_min_1.bi5", "1m")
                .expect("parsed")
                .0,
            PeriodKind::Day
        );
    }

    #[test]
    fn parse_path_wrong_timeframe_skipped() {
        let base = Path::new("/data/dukascopy/X");
        // H1 file must not match a D1 request.
        assert!(parsed(base, "2025/03/BID_candles_hour_1.bi5", "D1").is_none());
        assert!(parsed(base, "2025/BID_candles_day_1.bi5", "M1").is_none());
    }

    #[test]
    fn parse_path_bad_depth_or_names_skipped() {
        let base = Path::new("/data/dukascopy/X");
        // Year file nested too deep.
        assert!(parsed(base, "2025/03/BID_candles_day_1.bi5", "D1").is_none());
        // Non-numeric dirs.
        assert!(parsed(base, "YYYY/BID_candles_day_1.bi5", "D1").is_none());
        // Old-layout filename no longer matches.
        assert!(parsed(base, "EURUSD-D1-2025.bi5", "D1").is_none());
        // Non-bi5 extension.
        assert!(parsed(base, "2025/readme.txt", "D1").is_none());
    }

    // — should_skip_file —

    #[test]
    fn skip_file_no_filter() {
        let d = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        assert!(!skipped(d, PeriodKind::Day, None, None));
        assert!(!skipped(d, PeriodKind::Month, None, None));
        assert!(!skipped(d, PeriodKind::Year, None, None));
    }

    #[test]
    fn skip_file_before_start() {
        // file covers 2025-01-01 (day), start=2025-01-02 => skip
        let d = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        assert!(skipped(d, PeriodKind::Day, Some(pd("2025-01-02")), None));
        // day file ends at 2025-01-02, starts at 2025-01-01, start=2025-01-01 => not skip
        assert!(!skipped(d, PeriodKind::Day, Some(pd("2025-01-01")), None));
    }

    #[test]
    fn skip_file_after_end() {
        let d = NaiveDate::from_ymd_opt(2025, 1, 5).unwrap();
        assert!(skipped(d, PeriodKind::Day, None, Some(pd("2025-01-04"))));
    }

    #[test]
    fn skip_file_year_overlap() {
        let d = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        // year 2025 ends 2026-01-01, end=2025-06 => overlap
        assert!(!skipped(
            d,
            PeriodKind::Year,
            Some(pd("2025-06-01")),
            Some(pd("2025-06-30"))
        ));
        // year 2025 ends 2026-01-01, end=2024-12 => no overlap
        assert!(skipped(
            d,
            PeriodKind::Year,
            Some(pd("2024-01-01")),
            Some(pd("2024-12-31"))
        ));
    }

    #[test]
    fn period_end_out_of_range_errors() {
        // 旧代码此处 unwrap panic；现返回 Err，由管线计入硬失败。
        assert!(period_end(NaiveDate::MAX, PeriodKind::Day).is_err());
        assert!(period_end(NaiveDate::MAX, PeriodKind::Year).is_err());
        assert!(
            period_end(
                NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
                PeriodKind::Day
            )
            .is_ok()
        );
    }

    // — parse_bi5_records —

    fn make_d1_record(
        day_offset: u32,
        open: u32,
        close: u32,
        low: u32,
        high: u32,
        vol: u32,
    ) -> [u8; 24] {
        let mut rec = [0u8; 24];
        rec[0..4].copy_from_slice(&day_offset.to_be_bytes());
        rec[4..8].copy_from_slice(&open.to_be_bytes());
        rec[8..12].copy_from_slice(&close.to_be_bytes());
        rec[12..16].copy_from_slice(&low.to_be_bytes());
        rec[16..20].copy_from_slice(&high.to_be_bytes());
        rec[20..24].copy_from_slice(&vol.to_be_bytes());
        rec
    }

    #[test]
    fn parse_records_single() {
        let dt = NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let rec = make_d1_record(0, 100000, 100500, 99000, 101000, 5000);
        let (records, skipped) = parse_bi5_records(&rec, dt).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(records.len(), 1);
        assert!((records[0].open - 1.0).abs() < 0.0001);
        assert!((records[0].high - 1.01).abs() < 0.0001);
        assert!((records[0].low - 0.99).abs() < 0.0001);
        assert!((records[0].close - 1.005).abs() < 0.0001);
        assert!((records[0].volume - 5000.0).abs() < 0.001);
    }

    #[test]
    fn parse_records_multiple() {
        let dt = NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let r1 = make_d1_record(0, 100000, 100500, 99000, 101000, 5000);
        let r2 = make_d1_record(86400, 101000, 101500, 100000, 102000, 6000);
        let mut data = Vec::new();
        data.extend_from_slice(&r1);
        data.extend_from_slice(&r2);
        let (records, skipped) = parse_bi5_records(&data, dt).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].open_time + 86_400_000, records[1].open_time);
    }

    #[test]
    fn parse_records_empty() {
        let dt = NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let (records, skipped) = parse_bi5_records(&[], dt).unwrap();
        assert_eq!(skipped, 0);
        assert!(records.is_empty());
    }

    #[test]
    fn parse_records_partial_tail_warns() {
        let dt = NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        // 25 bytes = 1 full record + 1 byte
        let mut data = vec![0u8; 25];
        data[0..4].copy_from_slice(&0u32.to_be_bytes());
        data[4..8].copy_from_slice(&100000u32.to_be_bytes());
        data[8..12].copy_from_slice(&100500u32.to_be_bytes());
        data[12..16].copy_from_slice(&99000u32.to_be_bytes());
        data[16..20].copy_from_slice(&101000u32.to_be_bytes());
        data[20..24].copy_from_slice(&1000u32.to_be_bytes());
        let (records, skipped) = parse_bi5_records(&data, dt).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn parse_records_skips_invalid_ohlc() {
        // 高低错位（high < low）与零价同 binance 口径跳过并计数。
        let dt = NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let good = make_d1_record(0, 100000, 100500, 99000, 101000, 5000);
        let bad_hl = make_d1_record(86400, 100000, 100500, 101000, 99000, 5000);
        let bad_zero = make_d1_record(2 * 86400, 0, 0, 0, 0, 5000);
        let mut data = Vec::new();
        data.extend_from_slice(&good);
        data.extend_from_slice(&bad_hl);
        data.extend_from_slice(&bad_zero);
        let (records, skipped) = parse_bi5_records(&data, dt).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(skipped, 2);
    }

    // — load_single_bi5 (integration with real file) —

    #[test]
    fn load_real_eurusd_d1() {
        let path = Path::new("/data/data/dukascopy/EURUSD/2025/BID_candles_day_1.bi5");
        if !path.exists() {
            eprintln!("skipping: {path:?} not found");
            return;
        }
        let records = load_single_bi5(path).unwrap();
        assert!(!records.is_empty());
        assert!(records[0].open_time > 0);
        assert!(records[0].close > 0.0);
    }

    #[test]
    fn load_real_eurusd_m1() {
        let path = Path::new("/data/data/dukascopy/EURUSD/2024/06/01/BID_candles_min_1.bi5");
        if !path.exists() {
            eprintln!("skipping: {path:?} not found");
            return;
        }
        let records = load_single_bi5(path).unwrap();
        assert!(!records.is_empty());
        assert!(records[0].close > 0.0);
    }

    // — load_dukascopy_pipeline (integration) —

    fn setup_file(tmp: &TempDir, rel: &str) -> PathBuf {
        let path = tmp.path().join("dukascopy").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    }

    fn create_synthetic_bi5(path: &Path, num_records: u32) {
        use std::io::BufReader;
        let mut raw = Vec::with_capacity(num_records as usize * 24);
        for i in 0..num_records {
            let time_secs = i * (if num_records > 100 { 86400 } else { 60 }); // D1 or M1
            let mut rec = [0u8; 24];
            rec[0..4].copy_from_slice(&time_secs.to_be_bytes());
            rec[4..8].copy_from_slice(&100000u32.to_be_bytes());
            rec[8..12].copy_from_slice(&100500u32.to_be_bytes());
            rec[12..16].copy_from_slice(&99000u32.to_be_bytes());
            rec[16..20].copy_from_slice(&101000u32.to_be_bytes());
            rec[20..24].copy_from_slice(&1000u32.to_be_bytes());
            raw.extend_from_slice(&rec);
        }
        let mut input = BufReader::new(raw.as_slice());
        let mut compressed = Vec::new();
        lzma_rs::lzma_compress(&mut input, &mut compressed).unwrap();
        std::fs::write(path, &compressed).unwrap();
    }

    #[test]
    fn load_pipeline_single_file() {
        let tmp = TempDir::new().unwrap();
        let path = setup_file(&tmp, "EURUSD/2025/BID_candles_day_1.bi5");
        create_synthetic_bi5(&path, 100);
        let records =
            load_dukascopy_pipeline(tmp.path(), &sym("EURUSD"), &tf_d("D1"), None, None).unwrap();
        assert_eq!(records.len(), 100);
    }

    #[test]
    fn load_pipeline_corrupt_file_errors() {
        // 坏 bi5 是硬失败（零容忍）：即使有好文件，整批也 Err 并报告。
        let tmp = TempDir::new().unwrap();
        create_synthetic_bi5(&setup_file(&tmp, "EURUSD/2025/BID_candles_day_1.bi5"), 10);
        let bad = setup_file(&tmp, "EURUSD/2026/BID_candles_day_1.bi5");
        std::fs::write(&bad, b"this is not lzma").unwrap();
        let result = load_dukascopy_pipeline(tmp.path(), &sym("EURUSD"), &tf_d("D1"), None, None);
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("1 file(s) failed"), "unexpected: {msg}");
    }

    #[test]
    fn load_pipeline_ignores_other_timeframes() {
        let tmp = TempDir::new().unwrap();
        create_synthetic_bi5(&setup_file(&tmp, "EURUSD/2025/BID_candles_day_1.bi5"), 10);
        create_synthetic_bi5(
            &setup_file(&tmp, "EURUSD/2025/03/BID_candles_hour_1.bi5"),
            7,
        );
        let records =
            load_dukascopy_pipeline(tmp.path(), &sym("EURUSD"), &tf_d("D1"), None, None).unwrap();
        assert_eq!(records.len(), 10);
    }

    #[test]
    fn load_pipeline_date_filter() {
        let tmp = TempDir::new().unwrap();
        for day in 1..=3 {
            let rel = format!("EURUSD/2025/01/{day:02}/BID_candles_min_1.bi5");
            create_synthetic_bi5(&setup_file(&tmp, &rel), 5);
        }
        // filter to only day 2
        let records = load_dukascopy_pipeline(
            tmp.path(),
            &sym("EURUSD"),
            &tf_d("M1"),
            Some(pd("2025-01-02")),
            Some(pd("2025-01-02")),
        )
        .unwrap();
        assert_eq!(records.len(), 5);
        // verify timestamp is on 2025-01-02
        let expected = NaiveDate::from_ymd_opt(2025, 1, 2)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(records[0].open_time, expected);
    }

    #[test]
    fn load_pipeline_empty_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("dukascopy").join("EURUSD")).unwrap();
        let records =
            load_dukascopy_pipeline(tmp.path(), &sym("EURUSD"), &tf_d("D1"), None, None).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn load_pipeline_nonexistent_dir_errors() {
        let result =
            load_dukascopy_pipeline("/nonexistent/path", &sym("EURUSD"), &tf_d("D1"), None, None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ReaderError::Io(_) => {}
            _ => panic!("expected Io error"),
        }
    }

    #[test]
    fn dukascopy_reader_loads_data() {
        let tmp = TempDir::new().unwrap();
        let path = setup_file(&tmp, "EURUSD/2025/BID_candles_day_1.bi5");
        let mut raw = Vec::new();
        for i in 0..5u32 {
            let mut rec = [0u8; 24];
            rec[0..4].copy_from_slice(&(i * 86400).to_be_bytes());
            rec[4..8].copy_from_slice(&100000u32.to_be_bytes());
            rec[8..12].copy_from_slice(&100500u32.to_be_bytes());
            rec[12..16].copy_from_slice(&99000u32.to_be_bytes());
            rec[16..20].copy_from_slice(&101000u32.to_be_bytes());
            rec[20..24].copy_from_slice(&1000u32.to_be_bytes());
            raw.extend_from_slice(&rec);
        }
        create_synthetic_bi5_raw(&path, &raw);

        let records =
            load_dukascopy_pipeline(tmp.path(), &sym("EURUSD"), &tf_d("D1"), None, None).unwrap();
        assert_eq!(records.len(), 5);
    }

    fn create_synthetic_bi5_raw(path: &std::path::Path, records: &[u8]) {
        use std::io::BufReader;
        let mut input = BufReader::new(records);
        let mut compressed = Vec::new();
        lzma_rs::lzma_compress(&mut input, &mut compressed).unwrap();
        std::fs::write(path, &compressed).unwrap();
    }

    #[test]
    fn dukascopy_reader_empty_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("dukascopy").join("EURUSD")).unwrap();
        let records =
            load_dukascopy_pipeline(tmp.path(), &sym("EURUSD"), &tf_d("D1"), None, None).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn dukascopy_reader_missing_dir() {
        let result =
            load_dukascopy_pipeline("/nonexistent", &sym("EURUSD"), &tf_d("D1"), None, None);
        assert!(result.is_err());
    }
}
