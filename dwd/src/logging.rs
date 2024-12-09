use std::error::Error;

use log::LevelFilter;
use simple_logger::SimpleLogger;

pub fn init(verbosity: usize) -> Result<(), Box<dyn Error>> {
    let level = match verbosity {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };

    SimpleLogger::new()
        .with_level(LevelFilter::Off)
        .with_module_level("dwd", level)
        .with_module_level("actix_web", level)
        .with_module_level("dpdk", level)
        .with_utc_timestamps()
        .init()?;

    Ok(())
}
