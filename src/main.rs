use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = punch::cli::Cli::parse();
    punch::run(cli).await
}
