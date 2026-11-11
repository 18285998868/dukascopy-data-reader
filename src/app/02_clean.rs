//! 加载后清洗: 去重 + 缺口检查(只报不改数).
//!
//! - 去重: 输入须已按 `open_time` 排好序, 按时间戳 keep first, 返回去掉条数.
//! - 缺口: 按 `Timeframe::interval_ms` 扫描, 缺段只上报不补价不切段.

use crate::app::Record;
#[cfg(test)]
use crate::error::{ReaderError, Result};
#[cfg(test)]
use crate::types;
use crate::types::Timeframe;

/// 缺口报告留样上限, 与 binance reader 的 `MAX_SKIP_SAMPLES` 同口径.
const MAX_GAP_SAMPLES: usize = 5;

/// 兼容旧字符串入口：查单一真相源 `types::interval_ms_for`。
/// 新代码请直接用 `&Timeframe`（构造即带间隔，不会失败）。
#[cfg(test)]
pub(crate) fn interval_ms_for_timeframe(tf: &str) -> Result<i64> {
    types::interval_ms_for(tf)
        .ok_or_else(|| ReaderError::Parse(format!("unknown timeframe for gap check: {tf:?}")))
}

/// 缺口段: 缺口前后两根已知 bar 的时间戳.
/// `missing_bars` 为按期望间隔折算的缺数; `misaligned=true` 表示间隔不是期望
/// 间隔的整数倍(时间错位, 非单纯缺数, 此时 `missing_bars` 可能为 0 但仍需复核).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Gap {
    pub prev_time: i64,
    pub next_time: i64,
    pub missing_bars: i64,
    pub misaligned: bool,
}

/// 清洗报告: 去重数 + 缺口清单. 只供日志与复核, 下游序列不变.
#[derive(Debug, Default, Clone)]
pub(crate) struct CleanReport {
    pub dups_removed: usize,
    pub gaps: Vec<Gap>,
}

/// 去重: 输入须已按 `open_time` 排好序(`retain` 保序, keep first).
/// 返回(去重后, 去掉条数).
pub(crate) fn dedup_sorted(mut records: Vec<Record>) -> (Vec<Record>, usize) {
    let before = records.len();
    let mut last: Option<i64> = None;
    records.retain(|r| {
        if Some(r.open_time) == last {
            false
        } else {
            last = Some(r.open_time);
            true
        }
    });
    let removed = before - records.len();
    (records, removed)
}

/// 缺口扫描: 相邻两根间隔不等于期望间隔即记一段. 输入须已排序去重.
pub(crate) fn find_gaps(records: &[Record], interval_ms: i64) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for w in records.windows(2) {
        let diff = w[1].open_time - w[0].open_time;
        if diff != interval_ms {
            gaps.push(Gap {
                prev_time: w[0].open_time,
                next_time: w[1].open_time,
                missing_bars: (diff / interval_ms - 1).max(0),
                misaligned: diff % interval_ms != 0,
            });
        }
    }
    gaps
}

/// 一站式清洗: 去重 + 缺口检查. `Timeframe` 自带间隔，不再跳过检查.
pub(crate) fn clean_sorted(
    records: Vec<Record>,
    timeframe: &Timeframe,
) -> (Vec<Record>, CleanReport) {
    let (records, dups_removed) = dedup_sorted(records);
    let gaps = find_gaps(&records, timeframe.interval_ms());
    (records, CleanReport { dups_removed, gaps })
}

/// 缺口留样(行内 `tracing::warn(samples = ?...)` 用, 与坏行留样同 capped 风格).
pub(crate) fn gap_samples(gaps: &[Gap]) -> Vec<Gap> {
    gaps.iter().take(MAX_GAP_SAMPLES).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rec(t: i64) -> Record {
        Record {
            open_time: t,
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 100.0,
        }
    }

    fn tf(s: &str) -> Timeframe {
        Timeframe::parse(s).unwrap()
    }

    #[test]
    fn interval_maps_known_timeframes() {
        assert_eq!(interval_ms_for_timeframe("M1").unwrap(), 60_000);
        assert_eq!(interval_ms_for_timeframe("H1").unwrap(), 3_600_000);
        assert_eq!(interval_ms_for_timeframe("1d").unwrap(), 86_400_000);
        assert_eq!(interval_ms_for_timeframe("D1").unwrap(), 86_400_000);
        assert_eq!(interval_ms_for_timeframe("1m").unwrap(), 60_000);
    }

    #[test]
    fn interval_rejects_unknown_timeframe() {
        assert!(interval_ms_for_timeframe("3d").is_err());
        assert!(interval_ms_for_timeframe("").is_err());
    }

    #[test]
    fn dedup_keeps_first_and_counts() {
        let mut a = rec(100);
        a.close = 11.0;
        let mut b = rec(100);
        b.close = 99.0;
        let (out, removed) = dedup_sorted(vec![a, b, rec(200)]);
        assert_eq!(removed, 1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].close, 11.0);
    }

    #[test]
    fn gaps_detect_missing_bars() {
        let records = vec![rec(0), rec(60_000), rec(300_000)];
        let gaps = find_gaps(&records, 60_000);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].missing_bars, 3);
        assert!(!gaps[0].misaligned);
    }

    #[test]
    fn gaps_flag_misaligned_interval() {
        let records = vec![rec(0), rec(90_000)];
        let gaps = find_gaps(&records, 60_000);
        assert_eq!(gaps.len(), 1);
        assert!(gaps[0].misaligned);
    }

    #[test]
    fn gaps_empty_on_continuous_series() {
        let records = vec![rec(0), rec(60_000), rec(120_000)];
        assert!(find_gaps(&records, 60_000).is_empty());
    }

    #[test]
    fn clean_sorted_uses_timeframe_interval() {
        let (out, report) = clean_sorted(vec![rec(0), rec(60_000)], &tf("M1"));
        assert_eq!(out.len(), 2);
        assert!(report.gaps.is_empty());
        assert_eq!(report.dups_removed, 0);
    }
}
