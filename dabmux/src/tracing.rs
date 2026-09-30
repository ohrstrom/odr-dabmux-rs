use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

use crate::args::Args;

pub fn init(args: &Args) -> anyhow::Result<()> {
    let filter = if let Ok(env) = EnvFilter::try_from_default_env() {
        env
    } else {
        let spec = match args.verbose {
            0 => "info",
            1 => "info,dabmux=debug",
            _ => "trace,dabmux=trace",
        };

        EnvFilter::new(spec)
    };

    let show_level = filter
        .max_level_hint()
        .map(|lvl| lvl >= LevelFilter::DEBUG)
        .unwrap_or(false);

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_level(show_level)
        .with_target(show_level && args.verbose == 0)
        // .with_target(true)
        .init();

    Ok(())
}
