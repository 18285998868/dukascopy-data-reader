#[path = "02_clean.rs"]
mod clean;
#[path = "03_column.rs"]
pub mod columns;
#[path = "01_reader/dukascopy.rs"]
mod dukascopy;

use crate::config::DataConfig;
use crate::error::{ReaderError, Result};
use chrono::NaiveDate;
use rayon::prelude::*;

pub use columns::KlineColumns;

/// 文件级硬失败零容忍阈值（回测准确性优先）：坏文件数超过即整批 `Err`。
/// 行级坏行仍 warn 跳过（见 reader）；此处只管“整个文件不可读”
/// （损坏 bi5、非法文件名日期）。
/// 调大此值可回退为 warn 模式（仍打汇总 warn 供复核）。
pub(crate) const MAX_FAILED_FILES_TOLERATED: usize = 0;
/// 失败文件留样上限（与坏行/缺口留样同口径）。
const MAX_FAILED_SAMPLES: usize = 5;

/// 文件级硬失败收口：超阈值即 `Err`（附失败数+留样路径），容忍内则汇总 warn。
pub(crate) fn fail_on_hard_errors(source: &str, failed: &[String]) -> Result<()> {
    if failed.is_empty() {
        return Ok(());
    }
    let samples: Vec<&str> = failed
        .iter()
        .take(MAX_FAILED_SAMPLES)
        .map(String::as_str)
        .collect();
    if failed.len() > MAX_FAILED_FILES_TOLERATED {
        return Err(ReaderError::Parse(format!(
            "{source}: {} file(s) failed to load (tolerated {MAX_FAILED_FILES_TOLERATED}): {}",
            failed.len(),
            samples.join("; ")
        )));
    }
    tracing::warn!(
        source,
        failed_files = failed.len(),
        samples = ?samples,
        "loaded with failed files within tolerance; review data completeness"
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Record {
    pub open_time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// OHLC 行级校验：价格有限且 `> 0`，`volume` 有限且 `>= 0`，
/// 且 `high >= max(open, low, close)`、`low <= min(open, high, close)`。
/// 不满足即坏行（dukascopy 跳过计数）。
/// 直接比较：此处是“陈述值”校验而非计算结果，无需容差。
pub(crate) fn is_valid_ohlc(open: f64, high: f64, low: f64, close: f64, volume: f64) -> bool {
    for v in [open, high, low, close, volume] {
        if !v.is_finite() {
            return false;
        }
    }
    if open <= 0.0 || high <= 0.0 || low <= 0.0 || close <= 0.0 || volume < 0.0 {
        return false;
    }
    high >= low && high >= open && high >= close && low <= open && low <= close
}
/// 行级 UTC 日期裁剪：保留 `open_time` 落在
/// `[start 日 00:00, end 次日 00:00)` 的 bar。
///
/// 文件级过滤是快路径（dukascopy 按年/月周期），
/// 但年/月文件与区间部分重叠时会整文件保留，此处按行兜底，
/// 保证越界 bar 不进回测。`end_date` 语义为 UTC 当日含整天。
fn trim_to_date_range(
    records: Vec<Record>,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Vec<Record> {
    if start_date.is_none() && end_date.is_none() {
        return records;
    }
    let day_start_ms = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .expect("INVARIANT: NaiveDate always has a midnight instant")
            .and_utc()
            .timestamp_millis()
    };
    let start_ms = start_date.map(day_start_ms);
    // 含当日：上界为 end 次日 00:00（开区间）。
    let end_exclusive_ms = end_date.and_then(|d| d.succ_opt()).map(day_start_ms);
    records
        .into_iter()
        .filter(|r| {
            if let Some(lo) = start_ms
                && r.open_time < lo
            {
                return false;
            }
            if let Some(hi) = end_exclusive_ms
                && r.open_time >= hi
            {
                return false;
            }
            true
        })
        .collect()
}

/// 逻辑编排: 加载数据 -> 行级日期裁剪 -> 全局排序 -> clean -> 列转置.
///
/// `config` 构造即有效，无需二次 `validate()`（`parse-don't-validate`）。
/// 全局排序在此显式执行: `clean` 要求输入已按 `open_time` 有序.
pub fn load_columns(config: &DataConfig) -> Result<KlineColumns> {
    // `dir` 直接用 `Path`，支持非 UTF-8 路径（`api-impl-asref`）。
    let records = dukascopy::load_dukascopy_pipeline(
        config.dir(),
        config.symbol(),
        config.timeframe(),
        config.start_date(),
        config.end_date(),
    )?;

    let mut records = trim_to_date_range(records, config.start_date(), config.end_date());
    records.par_sort_by_key(|r| r.open_time);

    // 去重 + 缺口检查(只报不改数): 下游序列保持拼接原样, 报告供研究复核回测.
    let (records, report) = clean::clean_sorted(records, config.timeframe());
    if report.dups_removed > 0 {
        tracing::warn!(
            symbol = %config.symbol(),
            timeframe = %config.timeframe(),
            dups_removed = report.dups_removed,
            "deduped duplicate bars by open_time (keep first)"
        );
    }
    if !report.gaps.is_empty() {
        let missing_total: i64 = report.gaps.iter().map(|g| g.missing_bars.max(0)).sum();
        tracing::warn!(
            symbol = %config.symbol(),
            timeframe = %config.timeframe(),
            gap_segments = report.gaps.len(),
            missing_bars = missing_total,
            samples = ?clean::gap_samples(&report.gaps),
            "detected bar gaps; series kept as-is, review backtest results"
        );
    }

    Ok(KlineColumns::from(records))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(open_time: i64) -> Record {
        Record {
            open_time,
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 100.0,
        }
    }

    fn pd(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn day_ms(s: &str) -> i64 {
        pd(s)
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn valid_ohlc_boundaries() {
        // 平 bar（开高低收相等、零量）合法；NaN/错位非法。
        assert!(is_valid_ohlc(10.0, 10.0, 10.0, 10.0, 0.0));
        assert!(!is_valid_ohlc(10.0, 9.0, 12.0, 11.0, 1.0));
        assert!(!is_valid_ohlc(f64::NAN, 12.0, 9.0, 11.0, 1.0));
        assert!(!is_valid_ohlc(10.0, 12.0, 9.0, 11.0, f64::INFINITY));
    }

    #[test]
    fn trim_no_bounds_keeps_all() {
        let rows = vec![rec(0), rec(1), rec(i64::MAX), rec(i64::MIN)];
        assert_eq!(trim_to_date_range(rows, None, None).len(), 4);
    }

    #[test]
    fn trim_end_date_covers_whole_utc_day() {
        // end=2025-01-02：当日 23:59:59.999 保留，次日 00:00 丢弃。
        let rows = vec![
            rec(day_ms("2025-01-02")),
            rec(day_ms("2025-01-02") + 86_399_999),
            rec(day_ms("2025-01-03")),
        ];
        let out = trim_to_date_range(rows, None, Some(pd("2025-01-02")));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn trim_start_is_inclusive() {
        let rows = vec![rec(day_ms("2025-01-02") - 1), rec(day_ms("2025-01-02"))];
        let out = trim_to_date_range(rows, Some(pd("2025-01-02")), None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].open_time, day_ms("2025-01-02"));
    }

    #[test]
    fn trim_year_file_to_month_window() {
        // 年文件场景：全年 12 条月首 bar，窗口取 6 月只留 1 条。
        let rows: Vec<Record> = (1..=12)
            .map(|m| {
                rec(pd(&format!("2025-{m:02}-01"))
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc()
                    .timestamp_millis())
            })
            .collect();
        let out = trim_to_date_range(rows, Some(pd("2025-06-01")), Some(pd("2025-06-30")));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].open_time, day_ms("2025-06-01"));
    }
}
