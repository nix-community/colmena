//! Global CLI Setup.

use std::env;
use std::io;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use const_format::{concatcp, formatcp};
use tracing_subscriber::EnvFilter;

use crate::{
    command::{self, apply::DeployOpts},
    error::{ColmenaError, ColmenaResult},
    nix::{Hive, HivePath, hive::EvaluationMethod},
};

/// Base URL of the manual, without the trailing slash.
const MANUAL_URL_BASE: &str = "https://colmena.cli.rs";

/// URL to the manual.
///
/// We maintain CLI and Nix API stability for each minor version.
/// This ensures that the user always sees accurate documentations, and we can
/// easily perform updates to the manual after a release.
const MANUAL_URL: &str = concatcp!(
    MANUAL_URL_BASE,
    "/",
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);

/// The note shown when the user is using a pre-release version.
///
/// API stability cannot be guaranteed for pre-release versions.
/// Links to the version currently in development automatically
/// leads the user to the unstable manual.
const MANUAL_DISCREPANCY_NOTE: &str = "\nNote: You are using a pre-release version of Colmena, so the supported options may be different from what's in the manual.";

static LONG_ABOUT: &str = formatcp!(
    r#"NixOS deployment tool

Colmena helps you deploy to multiple hosts running NixOS.
For more details, read the manual at <{}>.

{}"#,
    MANUAL_URL,
    if !env!("CARGO_PKG_VERSION_PRE").is_empty() {
        MANUAL_DISCREPANCY_NOTE
    } else {
        ""
    }
);

static CONFIG_HELP: &str = formatcp!(
    r#"If this argument is not specified, Colmena will search upwards from the current working directory for a file named "flake.nix" or "hive.nix". This behavior is disabled if --config/-f is given explicitly.

For a sample configuration, check the manual at <{}>.
"#,
    MANUAL_URL
);

/// Display order in `--help` for arguments that should be shown first.
///
/// Currently reserved for -f/--config.
const HELP_ORDER_FIRST: usize = 100;

/// Display order in `--help` for arguments that are not very important.
const HELP_ORDER_LOW: usize = 2000;

/// When to display color.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ColorWhen {
    /// Detect automatically.
    #[default]
    Auto,

    /// Always display colors.
    Always,

    /// Never display colors.
    Never,
}

impl std::fmt::Display for ColorWhen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        })
    }
}

/// NixOS deployment tool
#[derive(Parser)]
#[command(
    name = "Colmena",
    bin_name = "colmena",
    author = "Zhaofeng Li <hello@zhaofeng.li>",
    version = env!("CARGO_PKG_VERSION"),
    long_about = LONG_ABOUT,
    max_term_width = 100,
)]
struct Opts {
    /// Path to a Hive expression, a flake.nix, or a Nix Flake URI
    #[arg(
        short = 'f',
        long,
        value_name = "CONFIG",
        long_help = CONFIG_HELP,
        display_order = HELP_ORDER_FIRST,
        global = true,
    )]
    config: Option<HivePath>,

    /// Show debug information for Nix commands
    ///
    /// Passes --show-trace to Nix commands
    #[arg(long, global = true)]
    show_trace: bool,

    /// Allow impure expressions
    ///
    /// Passes --impure to Nix commands
    #[arg(long, global = true)]
    impure: bool,

    /// Passes an arbitrary option to Nix commands
    ///
    /// This only works when building locally.
    #[arg(
        long,
        global = true,
        num_args = 2,
        value_names = ["NAME", "VALUE"],
    )]
    nix_option: Vec<String>,

    /// Use legacy flake evaluation (deprecated)
    ///
    /// If enabled, flakes will be evaluated using `builtins.getFlake` with the `nix-instantiate` CLI.
    #[arg(long, default_value_t, global = true, hide = true)]
    legacy_flake_eval: bool,

    /// This flag no longer has an effect
    ///
    /// Previously, it enabled direct flake evaluation which is now the default.
    #[arg(
        long = "experimental-flake-eval",
        default_value_t,
        global = true,
        hide = true
    )]
    deprecated_experimental_flake_eval_flag: bool,

    /// When to colorize the output
    ///
    /// By default, Colmena enables colorized output when the terminal supports it.
    ///
    /// It's also possible to specify the preference using environment variables. See
    /// <https://bixense.com/clicolors>.
    #[arg(
        long,
        value_name = "WHEN",
        default_value_t,
        global = true,
        display_order = HELP_ORDER_LOW,
    )]
    color: ColorWhen,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Apply(command::apply::Opts),

    ApplyLocal(command::apply_local::Opts),

    /// Build configurations but not push to remote machines
    ///
    /// This subcommand behaves as if you invoked `apply` with the `build` goal.
    Build {
        #[command(flatten)]
        deploy: DeployOpts,
    },

    Eval(command::eval::Opts),

    /// Upload keys to remote hosts
    ///
    /// This subcommand behaves as if you invoked `apply` with the pseudo `keys` goal.
    UploadKeys {
        #[command(flatten)]
        deploy: DeployOpts,
    },

    Exec(command::exec::Opts),

    /// Start an interactive REPL with the complete configuration
    ///
    /// In the REPL, you can inspect the configuration interactively with tab
    /// completion. The node configurations are accessible under the `nodes`
    /// attribute set.
    Repl,

    /// Show information about the current Nix installation
    NixInfo,

    /// Run progress spinner tests
    #[cfg(debug_assertions)]
    #[command(hide = true)]
    TestProgress,

    /// Generate shell auto-completion files (Internal)
    #[command(hide = true)]
    GenCompletions {
        shell: Shell,
    },
}

async fn get_hive(opts: &Opts) -> ColmenaResult<Hive> {
    let path = match &opts.config {
        Some(path) => path.clone(),
        None => {
            // traverse upwards until we find hive.nix
            let mut cur = std::env::current_dir()?;
            let mut file_path = None;

            loop {
                let flake = cur.join("flake.nix");
                if flake.is_file() {
                    file_path = Some(flake);
                    break;
                }

                let legacy = cur.join("hive.nix");
                if legacy.is_file() {
                    file_path = Some(legacy);
                    break;
                }

                match cur.parent() {
                    Some(parent) => {
                        cur = parent.to_owned();
                    }
                    None => {
                        break;
                    }
                }
            }

            if file_path.is_none() {
                tracing::error!(
                    "Could not find `hive.nix` or `flake.nix` in {:?} or any parent directory",
                    std::env::current_dir()?
                );
            }

            HivePath::from_path(file_path.unwrap()).await?
        }
    };

    match &path {
        HivePath::Legacy(p) => {
            tracing::info!("Using configuration: {}", p.to_string_lossy());
        }
        HivePath::Flake(flake) => {
            tracing::info!("Using flake: {}", flake.uri());
        }
    }

    let mut hive = Hive::new(path).await?;

    if opts.show_trace {
        hive.set_show_trace(true);
    }

    if opts.impure {
        hive.set_impure(true);
    }

    if opts.deprecated_experimental_flake_eval_flag {
        tracing::error!(
            "--experimental-flake-eval is now the default and this flag no longer has an effect"
        );
        return Err(ColmenaError::Unsupported);
    }

    if opts.legacy_flake_eval {
        tracing::warn!("Using legacy flake eval (deprecated)");
        tracing::warn!(
            r#"Consider upgrading to the new evaluator by adding Colmena as an input and expose the `colmenaHive` output:
  outputs = {{ self, colmena, ... }}: {{
    colmenaHive = colmena.lib.makeHive self.outputs.colmena;
    colmena = ...;
    }};
"#
        );
        hive.set_evaluation_method(EvaluationMethod::NixInstantiate);
    }

    for chunks in opts.nix_option.chunks_exact(2) {
        let [name, value] = chunks else {
            unreachable!()
        };
        hive.add_nix_option(name.clone(), value.clone());
    }

    Ok(hive)
}

pub async fn run() {
    let opts = Opts::parse();

    set_color_pref(&opts.color);
    init_logging();

    if let Command::GenCompletions { shell } = opts.command {
        print_completions(shell, &mut Opts::command());
        return;
    }

    let hive = get_hive(&opts).await.expect("Failed to get flake or hive");

    use crate::troubleshooter::run_wrapped as r;

    match opts.command {
        Command::Apply(args) => r(command::apply::run(hive, args), opts.config).await,
        Command::ApplyLocal(args) => r(command::apply_local::run(hive, args), opts.config).await,
        Command::Eval(args) => r(command::eval::run(hive, args), opts.config).await,
        Command::Exec(args) => r(command::exec::run(hive, args), opts.config).await,
        Command::NixInfo => r(command::nix_info::run(), opts.config).await,
        Command::Repl => r(command::repl::run(hive), opts.config).await,
        #[cfg(debug_assertions)]
        Command::TestProgress => r(command::test_progress::run(), opts.config).await,
        Command::Build { deploy } => {
            let args = command::apply::Opts {
                deploy,
                goal: crate::nix::Goal::Build,
            };
            r(command::apply::run(hive, args), opts.config).await
        }
        Command::UploadKeys { deploy } => {
            let args = command::apply::Opts {
                deploy,
                goal: crate::nix::Goal::UploadKeys,
            };
            r(command::apply::run(hive, args), opts.config).await
        }
        Command::GenCompletions { .. } => unreachable!(),
    }
}

fn print_completions(shell: Shell, cmd: &mut clap::Command) {
    let bin_name = cmd
        .get_bin_name()
        .expect("Must have a bin_name")
        .to_string();

    clap_complete::generate(shell, cmd, bin_name, &mut std::io::stdout());
}

fn set_color_pref(when: &ColorWhen) {
    if when != &ColorWhen::Auto {
        clicolors_control::set_colors_enabled(when == &ColorWhen::Always);
    }
}

fn init_logging() {
    let colors_enabled = clicolors_control::colors_enabled();
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .with_writer(io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .without_time()
        .with_ansi(colors_enabled)
        .init();
}
