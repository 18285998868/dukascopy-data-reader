//! Integration tests against real `.bi5` files copied from Dukascopy.
//!
//! Fixtures use the downloader layout with a zero-based month directory:
//! `2023/06` holds July 2023 hourly candles.

use std::path::PathBuf;

use chrono::NaiveDate;
use dukascopy_data_reader::{DataConfig, PriceType, Timeframe, load, load_columns};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid fixture date")
}

fn config(timeframe: Timeframe, start: NaiveDate, end: NaiveDate) -> DataConfig {
    DataConfig::new(fixtures(), "EURUSD", timeframe)
        .expect("valid fixture config")
        .with_price_type(PriceType::Bid)
        .with_date_range(Some(start), Some(end))
        .expect("valid fixture range")
}

#[test]
fn daily_2025_matches_the_archive() {
    let loaded = load(&config(Timeframe::D1, date(2025, 1, 1), date(2025, 12, 31)))
        .expect("daily fixture loads");

    let columns = loaded.columns();
    assert_eq!(columns.len(), 365);
    assert_eq!(columns.time()[0], 1_735_689_600_000); // 2025-01-01T00:00:00Z
    assert_eq!(columns.time()[364], 1_767_139_200_000); // 2025-12-31T00:00:00Z
    assert_eq!(columns.open()[0], 1.035_03);
    assert_eq!(columns.high()[0], 1.035_77);
    assert_eq!(columns.low()[0], 1.034_62);
    assert_eq!(columns.close()[0], 1.035_07);
    assert_eq!(columns.volume()[0], 1_169_691_832);

    let report = loaded.report();
    assert_eq!(report.requested_files(), 1);
    assert_eq!(report.loaded_files(), 1);
    assert!(report.missing_files().is_empty());
    assert_eq!(report.rows_decoded(), 365);
    assert_eq!(report.rows_returned(), 365);
    assert_eq!(report.rows_trimmed(), 0);
    assert_eq!(report.rows_invalid(), 0);
    assert_eq!(report.rows_out_of_bucket(), 0);
    assert_eq!(report.duplicates_removed(), 0);
    assert_eq!(report.first_bar_ms(), Some(1_735_689_600_000));
    assert_eq!(report.last_bar_ms(), Some(1_767_139_200_000));
    // Dukascopy stores weekend and holiday bars as zero-volume flat candles,
    // so the daily series is continuous and complete.
    assert_eq!(report.gaps().segments(), 0);
    assert!(report.is_complete());
}

#[test]
fn hourly_july_2023_is_stored_under_month_06() {
    let loaded = load(&config(Timeframe::H1, date(2023, 7, 1), date(2023, 7, 31)))
        .expect("hourly fixture loads");

    let columns = loaded.columns();
    assert_eq!(columns.len(), 744);
    assert_eq!(columns.time()[0], 1_688_169_600_000); // 2023-07-01T00:00:00Z
    assert_eq!(columns.time()[743], 1_690_844_400_000); // 2023-07-31T23:00:00Z
    assert_eq!(columns.open()[0], 1.090_95);
    assert_eq!(columns.close()[743], 1.099_66);
    assert_eq!(columns.volume()[0], 0);

    let report = loaded.report();
    assert_eq!(report.requested_files(), 1);
    assert_eq!(report.loaded_files(), 1);
    assert!(report.missing_files().is_empty());
    assert_eq!(report.rows_decoded(), 744);
    assert_eq!(report.rows_returned(), 744);
    assert_eq!(report.rows_invalid(), 0);
    assert_eq!(report.rows_out_of_bucket(), 0);
    assert_eq!(report.first_bar_ms(), Some(1_688_169_600_000));
    assert_eq!(report.last_bar_ms(), Some(1_690_844_400_000));
    assert_eq!(report.gaps().segments(), 0);
    assert!(report.is_complete());
}

#[test]
fn load_columns_matches_load() {
    let cfg = config(Timeframe::D1, date(2025, 1, 1), date(2025, 12, 31));
    let columns = load_columns(&cfg).expect("columns load");
    let loaded = load(&cfg).expect("full load");
    assert_eq!(columns, *loaded.columns());
}

#[test]
fn wrong_zero_based_month_reports_a_missing_file() {
    // February 2023 maps to `2023/01`, which the fixture does not contain.
    let loaded = load(&config(Timeframe::H1, date(2023, 2, 1), date(2023, 2, 28)))
        .expect("missing files are reported, not fatal");
    assert!(loaded.columns().is_empty());
    assert_eq!(loaded.report().requested_files(), 1);
    assert_eq!(loaded.report().loaded_files(), 0);
    assert_eq!(loaded.report().missing_files().len(), 1);
    assert_eq!(
        loaded.report().missing_files()[0].bucket().to_string(),
        "2023/01"
    );
    assert!(!loaded.report().is_complete());
}
