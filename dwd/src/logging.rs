use std::{error::Error, io::stderr};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init(verbosity: usize) -> Result<(), Box<dyn Error>> {
    let level = match verbosity {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    let filter_layer = Targets::new()
        .with_target("dwd", level)
        .with_target("dpdk", level)
        .with_default(LevelFilter::WARN.min(level));
    let fmt_layer = tracing_subscriber::fmt::layer().with_writer(stderr);
    let registry = tracing_subscriber::registry().with(filter_layer).with(fmt_layer);

    registry.try_init()?;

    Ok(())
}
