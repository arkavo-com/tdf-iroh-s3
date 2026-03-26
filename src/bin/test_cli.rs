use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "tdf-iroh-s3-test", about = "Test CLI for tdf-iroh-s3 nodes")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a TDF file locally
    CreateTdf {
        #[arg(short, long)]
        attribute: String,

        #[arg(short, long, default_value = "test payload")]
        data: String,

        #[arg(short, long)]
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::CreateTdf {
            attribute,
            data,
            output,
        } => {
            tdf_iroh_s3::test_cli::create_tdf::create_tdf_file(
                &attribute,
                data.as_bytes(),
                &output,
            )?;
        }
    }

    Ok(())
}
