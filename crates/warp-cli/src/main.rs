use clap::{Parser, Subcommand};

mod commands;
mod templates;

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
    /// Package a project as a Wasm component.
    ///
    /// Supported languages: rust, go, js, typescript, bun.
    ///
    /// Language is read from [build].lang in warp.toml, or auto-detected
    /// from project marker files (bunfig.toml → bun, Cargo.toml → rust,
    /// go.mod → go, package.json → typescript/js). Use --lang to override.
    Pack {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        path: String,
        /// Override the build language (rust, go, js, typescript, bun).
        /// If not specified, reads from warp.toml or auto-detects.
        #[arg(short, long)]
        lang: Option<String>,
    },
    /// Scaffold a new WarpGrid project from a template.
    ///
    /// Available templates: async-rust, async-go, async-ts
    Init {
        /// Template name (async-rust, async-go, async-ts)
        #[arg(short, long)]
        template: String,
        /// Target directory (default: ./<template-name>)
        #[arg(short, long)]
        path: Option<String>,
    },
    /// Login to WarpGrid Cloud or register a new account.
    Login {
        /// API key (for existing accounts).
        #[arg(long)]
        api_key: Option<String>,
        /// Email address (to register a new account).
        #[arg(long)]
        email: Option<String>,
        /// Cloud API URL (default: http://localhost:8443).
        #[arg(long)]
        api_url: Option<String>,
    },
    /// Deploy a Wasm component to WarpGrid Cloud.
    Deploy {
        /// Project directory (default: current directory).
        #[arg(short, long, default_value = ".")]
        path: String,
        /// Target region (default: iad).
        #[arg(short, long)]
        region: Option<String>,
        /// Override build language.
        #[arg(short, long)]
        lang: Option<String>,
    },
    /// Show deployment status.
    Status,
    /// Destroy a deployment.
    Destroy {
        /// Deployment ID to destroy.
        deployment_id: String,
    },
    /// Show WarpGrid Cloud platform status.
    Ping {
        /// Cloud API URL (overrides config).
        #[arg(long)]
        api_url: Option<String>,
    },
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
        /// Override the project language (rust, go, typescript, bun).
        /// If not specified, auto-detects from project files.
        #[arg(short, long)]
        lang: Option<String>,
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
            ConvertAction::Analyze { path, format, lang } => {
                let has_blockers = commands::convert::analyze(&path, &format, lang.as_deref())?;
                if has_blockers {
                    std::process::exit(1);
                }
                Ok(())
            }
            ConvertAction::Init { path } => {
                commands::convert::init(&path)
            }
        },
        Commands::Pack { path, lang } => {
            commands::pack::pack(&path, lang.as_deref())
        }
        Commands::Init { template, path } => {
            commands::init::init(&template, path.as_deref())
        }
        Commands::Login { api_key, email, api_url } => {
            commands::cloud::login(
                api_key.as_deref(),
                api_url.as_deref(),
                email.as_deref(),
            )
        }
        Commands::Deploy { path, region, lang } => {
            commands::cloud::deploy(&path, region.as_deref(), lang.as_deref())
        }
        Commands::Status => {
            commands::cloud::status()
        }
        Commands::Destroy { deployment_id } => {
            commands::cloud::destroy(&deployment_id)
        }
        Commands::Ping { api_url } => {
            commands::cloud::platform_status(api_url.as_deref())
        }
    }
}
