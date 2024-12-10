//! TODO:
//! 1. Refactor DPDK worker.
//! 2. Refactor DPDK library.
//! 3. Try different logging libraries. How to combine it with UI?

use core::error::Error;

use clap::Parser;
use dwd::{cfg::Config, cmd::Cmd, runtime::Runtime};
use tokio::runtime::Builder;

pub fn main() {
    let cmd = Cmd::parse();
    dwd::logging::init(cmd.verbose as usize).unwrap();

    if let Err(err) = run(cmd) {
        log::error!("ERROR: {err}");
        std::process::exit(1);
    }
}

fn run(cmd: Cmd) -> Result<(), Box<dyn Error>> {
    let cfg: Config = cmd.try_into()?;

    // Init I/O runtime.
    Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .thread_name("runtime")
        .build()?
        .block_on(async {
            let runtime = Runtime::new(cfg);

            runtime.run().await
        })
}
