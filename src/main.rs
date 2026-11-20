use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use swagger_rust::config::{self, DEFAULT_CONFIG_FILE};
use swagger_rust::Options;

/// Generates typed Rust API clients from OpenAPI 3 / Swagger 2 documents.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The config file.
    #[arg(short, long, default_value = DEFAULT_CONFIG_FILE, global = true)]
    config: PathBuf,
    /// Generate from the `swagger.json` saved in each output directory
    /// (see `keepJson`) instead of downloading.
    #[arg(long)]
    local: bool,
    /// Write nothing; exit with 1 if the generated files are out of date.
    #[arg(long)]
    check: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Write a starter config file.
    Init,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    if let Some(Command::Init) = cli.command {
        if cli.config.exists() {
            bail!("{} already exists", cli.config.display());
        }
        std::fs::write(&cli.config, config::STARTER)?;
        println!(
            "Wrote {}: set `url` and `dir`, then run swagger-rust.",
            cli.config.display()
        );
        return Ok(ExitCode::SUCCESS);
    }

    let configs = config::load(&cli.config)?;
    let options = Options {
        local: cli.local,
        check: cli.check,
    };
    let mut failed = false;
    let mut stale = false;
    for config in &configs {
        println!("{} → {}", config.url, config.dir.display());
        match swagger_rust::generate(config, options) {
            Ok(report) => {
                for warning in &report.warnings {
                    println!("  warning: {warning}");
                }
                for path in &report.removed {
                    println!(
                        "  {} {}",
                        if cli.check { "would remove" } else { "removed" },
                        path.display()
                    );
                }
                if cli.check && !report.changed.is_empty() {
                    stale = true;
                    for path in &report.changed {
                        println!("  out of date: {}", path.display());
                    }
                }
                println!(
                    "  {} endpoints, {} types; {} files {}, {} unchanged",
                    report.endpoints,
                    report.types,
                    report.changed.len(),
                    if cli.check { "out of date" } else { "written" },
                    report.unchanged
                );
            }
            Err(error) => {
                failed = true;
                eprintln!("  error: {error:#}");
            }
        }
    }
    Ok(if failed || stale {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
