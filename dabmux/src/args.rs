use clap::Parser;

use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version)]
pub struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    #[arg(long, default_value = "7777")]
    pub port: u16,

    #[arg(long = "config", short = 'c')]
    pub config: Option<PathBuf>,

    #[arg(long)]
    pub watch_config: bool,

    /// Verbosity (-v, -vv, -vvv)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    pub verbose: u8,
}
