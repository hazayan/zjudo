use zjudo::cli;

fn main() {
    // Parse command line arguments
    let cli = cli::parse_args();

    // Initialize logging
    init_logging(cli.verbose, cli.debug);

    // Run the CLI application
    if let Err(e) = cli::run(cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

/// Initialize logging based on verbosity
fn init_logging(verbose: bool, debug: bool) {
    let log_level = if debug {
        log::LevelFilter::Debug
    } else if verbose {
        log::LevelFilter::Info
    } else {
        log::LevelFilter::Warn
    };

    env_logger::Builder::new()
        .filter_level(log_level)
        .format_timestamp(None)
        .format_module_path(false)
        .init();
}
