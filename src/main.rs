use clap::Parser;
use colored::Colorize;
use synapse::cli::Cli;

fn main() {
    match synapse::core::switcher::try_exec_shim() {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");

    rt.block_on(async {
        let cli = Cli::parse();
        if let Err(e) = synapse::cli::run(cli).await {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    });
}
