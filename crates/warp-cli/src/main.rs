use clap::{Parser, Subcommand};

mod commands;

#[derive(Parser)]
#[command(
    name = "warp",
    about = "WarpGrid — Wasm-native cluster orchestrator",
    version,
    propagate_version = true,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze a project for Wasm compatibility
    Convert {
        #[command(subcommand)]
        action: ConvertAction,
    },
    /// Package a project as a Wasm component
    Pack {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        path: String,

        /// Override language (default: read from warp.toml)
        #[arg(short, long, value_parser = ["rust", "go", "typescript", "bun"])]
        lang: Option<String>,
    },
    // Phase 3+:
    // Deploy { ... },
    // Status { ... },
    // Logs { ... },
    // Scale { ... },
    // Nodes { ... },
}

#[derive(Subcommand)]
enum ConvertAction {
    /// Analyze a project for Wasm compatibility
    Analyze {
        /// Path to project directory or Dockerfile
        #[arg(short, long, default_value = ".")]
        path: String,
        /// Output format: text or json
        #[arg(short, long, default_value = "text")]
        format: String,
    },
    /// Generate a warp.toml scaffold from analysis
    Init {
        #[arg(short, long, default_value = ".")]
        path: String,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("warp=info".parse()?)
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Convert { action } => match action {
            ConvertAction::Analyze { path, format } => {
                commands::convert::analyze(&path, &format)
            }
            ConvertAction::Init { path } => {
                commands::convert::init(&path)
            }
        },
        Commands::Pack { path, lang } => {
            commands::pack::pack(&path, lang.as_deref())
        }
    }
}
