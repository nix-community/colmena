use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Args;

use crate::error::ColmenaError;
use crate::nix::{deployment::EvaluationNodeLimit, node_filter::NodeFilterOpts, Hive, NodeName};

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

    /// Evaluation node limit
    ///
    /// Limits the maximum number of hosts to be evaluated at once when using --on.
    ///
    /// Set to 0 to disable the limit.
    #[arg(value_name = "LIMIT", default_value_t, long)]
    eval_node_limit: EvaluationNodeLimit,

    /// Actually instantiate the expression
    #[arg(long)]
    instantiate: bool,

    #[command(flatten)]
    node_filter: NodeFilterOpts,

    /// The .nix file containing the expression
    #[arg(value_name = "FILE", conflicts_with("expression"))]
    expression_file: Option<PathBuf>,
}

pub async fn run(
    hive: Hive,
    Opts {
        expression,
        eval_node_limit,
        instantiate,
        node_filter,
        expression_file,
    }: Opts,
) -> Result<(), ColmenaError> {
    let expression = expression_file
        .map(|path| {
            format!(
                "import {}",
                path.canonicalize()
                    .expect("Could not generate absolute path to expression file.")
                    .to_str()
                    .unwrap()
            )
        })
        .or(expression);

    match (expression, node_filter.on) {
        (Some(expression), None) => {
            let result = hive.introspect(expression, instantiate).await?;

            if instantiate {
                print!("{}", result);
            } else {
                println!("{}", result);
            }
        }
        (None, Some(node_filter)) => {
            if instantiate {
                tracing::error!("--instantiate cannot be used with --on.");
                quit::with_code(1);
            }

            eval_selected_nodes(hive, node_filter, eval_node_limit).await?;
        }
        (Some(_), Some(_)) => {
            tracing::error!("--on cannot be used with an ad hoc expression.");
            quit::with_code(1);
        }
        (None, None) => {
            tracing::error!(
                "Provide either an expression (-E), a .nix file, or a node selector with --on."
            );
            quit::with_code(1);
        }
    };

    Ok(())
}

async fn eval_selected_nodes(
    hive: Hive,
    node_filter: crate::nix::NodeFilter,
    eval_node_limit: EvaluationNodeLimit,
) -> Result<(), ColmenaError> {
    let targets = hive.select_nodes(Some(node_filter), None, false).await?;

    let mut nodes: Vec<NodeName> = targets.into_keys().collect();
    nodes.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    let Some(chunk_size) = eval_node_limit.get_limit() else {
        return print_selected_nodes(hive.eval_selected(&nodes, None).await?);
    };

    let mut results = BTreeMap::new();
    for chunk in nodes.chunks(chunk_size) {
        results.extend(to_printable_paths(hive.eval_selected(chunk, None).await?));
    }

    println!("{}", serde_json::to_string_pretty(&results).unwrap());
    Ok(())
}

fn print_selected_nodes(
    results: std::collections::HashMap<NodeName, crate::nix::ProfileDerivation>,
) -> Result<(), ColmenaError> {
    let results = to_printable_paths(results);
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
    Ok(())
}

fn to_printable_paths(
    results: std::collections::HashMap<NodeName, crate::nix::ProfileDerivation>,
) -> BTreeMap<String, String> {
    results
        .into_iter()
        .map(|(name, drv)| {
            (
                name.as_str().to_string(),
                drv.as_store_path().as_path().display().to_string(),
            )
        })
        .collect()
}
