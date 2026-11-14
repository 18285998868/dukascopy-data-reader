//! Load one TOML request and print a deterministic summary.
//!
//! Used by the smoke step in `STEP.md`; exits non-zero when the load is not
//! complete (missing files, invalid rows, duplicates or gaps).
//!
//! ```text
//! cargo run --release --example load -- config/00.smoke.toml
//! ```

use dukascopy_data_reader::{DataConfig, load};
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A second initialization can only happen when the example is embedded;
    // there is nothing to recover from, so the error is intentionally ignored.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let Some(path) = std::env::args_os().nth(1) else {
        return Err("usage: load <config.toml>".into());
    };
    let text = std::fs::read_to_string(&path)?;
    let config: DataConfig = toml::from_str(&text)?;
    tracing::info!(
        root = %config.root().display(),
        symbol = %config.symbol(),
        timeframe = %config.timeframe(),
        price_type = %config.price_type(),
        start = ?config.start_date(),
        end = ?config.end_date(),
        "loaded config"
    );
    let loaded = load(&config)?;
    let report = loaded.report();

    println!(
        "symbol={} timeframe={} price_type={} bars={} files={}/{} missing={} \
         invalid={} out_of_bucket={} duplicates={} gaps={} first={:?} last={:?} complete={}",
        config.symbol(),
        config.timeframe(),
        config.price_type(),
        loaded.columns().len(),
        report.loaded_files(),
        report.requested_files(),
        report.missing_files().len(),
        report.rows_invalid(),
        report.rows_out_of_bucket(),
        report.duplicates_removed(),
        report.gaps().segments(),
        report.first_bar_ms(),
        report.last_bar_ms(),
        report.is_complete(),
    );

    if report.is_complete() {
        Ok(())
    } else {
        Err(format!(
            "incomplete load: {} missing file(s), {} invalid row(s), {} gap segment(s)",
            report.missing_files().len(),
            report.rows_invalid(),
            report.gaps().segments()
        )
        .into())
    }
}
