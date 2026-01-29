mod args;
mod commands;

pub use args::*;
pub use commands::*;

/// Parse command line arguments
pub fn parse_args() -> Cli {
    args::parse_args()
}

/// Run the CLI application
pub fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Boot(args) => commands::boot(args, cli.verbose, cli.debug),
        Commands::Load(args) => commands::load(args, cli.verbose, cli.debug),
        Commands::Unload(args) => commands::unload(args, cli.verbose, cli.debug),
        Commands::List(args) => commands::list(args, cli.verbose, cli.debug),
        Commands::Info(args) => commands::info(args, cli.verbose, cli.debug),
        Commands::Test(args) => commands::test(args, cli.verbose, cli.debug),
        Commands::Config(args) => commands::config(args, cli.verbose, cli.debug),
    }
}
