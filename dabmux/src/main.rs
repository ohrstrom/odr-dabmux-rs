mod app;
mod args;
mod tracing;

mod api;
mod config;
mod fic;
mod runtime;
#[cfg(test)]
mod testsupport;
mod timing;

use crate::app::App;
use crate::args::Args;
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing::init(&args)?;

    App::new(args).await?.run().await
}
