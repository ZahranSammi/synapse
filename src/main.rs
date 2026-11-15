use clap::Parser;
use colored::Colorize;
use synapse::cli::Cli;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = synapse::cli::run(cli).await {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}
