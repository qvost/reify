use reify_cli::{Cli, CliError, commands};
use std::{io::IsTerminal, process::ExitCode, time::Duration};
fn main() -> ExitCode {
    let cli = match Cli::parse_checked_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    let color = reify_cli::ui::use_color(cli.color);
    colored::control::set_override(color);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    if let Err(error) = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(
            cli.color == reify_cli::Color::Always || (color && std::io::stderr().is_terminal()),
        )
        .with_writer(std::io::stderr)
        .try_init()
    {
        eprintln!("reify: cannot initialize logging: {error}");
        return ExitCode::FAILURE;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("reify: cannot start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(commands::run_with_shutdown(
        cli,
        commands::shutdown_signal(),
    ));
    // Cancel remaining HTTP tasks before waiting for independent browser reapers.
    runtime.shutdown_timeout(Duration::from_secs(5));
    let cleanup = reify_probe::process::wait_for_cleanup(Duration::from_secs(2));
    let result = match (result, cleanup) {
        (Ok(()), Err(error)) => Err(CliError::Core(error)),
        (result, _) => result,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Interrupted) => {
            eprintln!("reify: interrupted");
            ExitCode::from(130)
        }
        Err(CliError::Io(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("reify: {error}");
            ExitCode::FAILURE
        }
    }
}
