use clap::Parser;
use veil::cli::{dispatch, Cli};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = dispatch(cli).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
