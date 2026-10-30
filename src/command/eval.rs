use std::path::PathBuf;

use clap::Args;

use crate::error::ColmenaError;
use crate::nix::Hive;

/// Evaluate an expression using the complete configuration
///
/// Your expression should take an attribute set with keys `pkgs`, `lib` and `nodes` (like a NixOS
/// module) and return a JSON-serializable value. For example, to retrieve the configuration of one
/// node, you may write something like:
///
///    { nodes, ... }: nodes.node-a.config.networking.hostName
#[derive(Debug, Args)]
#[command(name = "eval", alias = "introspect")]
pub struct Opts {
    /// The Nix expression
    #[arg(short = 'E', value_name = "EXPRESSION")]
    expression: Option<String>,

    /// Actually instantiate the expression
    #[arg(long)]
    instantiate: bool,

    /// The .nix file containing the expression
    #[arg(value_name = "FILE", conflicts_with("expression"))]
    expression_file: Option<PathBuf>,
}

pub async fn run(
    hive: Hive,
    Opts {
        expression,
        instantiate,
        expression_file,
    }: Opts,
) -> Result<(), ColmenaError> {
    let expression = expression_file
        .map(|path| {
            path.canonicalize()
                .and_then(|absolute| {
                    absolute.into_os_string().into_string().map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "path is not valid UTF-8",
                        )
                    })
                })
                .map(|absolute| format!("import {}", absolute))
                .map_err(|error| ColmenaError::ExpressionFileError { path, error })
        })
        .transpose()?
        .or(expression);

    let Some(expression) = expression else {
        tracing::error!(
            "Provide either an expression (-E) or a .nix file containing an expression."
        );
        quit::with_code(1);
    };

    let result = hive.introspect(expression, instantiate).await?;

    if instantiate {
        print!("{}", result);
    } else {
        println!("{}", result);
    }

    Ok(())
}
