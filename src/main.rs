use core::error::Error;

use clap::Parser;
use dwd::{cfg::Config, cmd::Cmd, runtime::Runtime};
use tokio::runtime::Builder;

pub fn main() {
    let cmd = Cmd::parse();

    if let Err(err) = run(cmd) {
        eprintln!("ERROR: {err}");
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
