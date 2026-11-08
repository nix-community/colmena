mod assets;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::convert::AsRef;
use std::path::{Path, PathBuf};

use const_format::formatcp;
use itertools::Itertools;
use tokio::process::Command;
use tokio::sync::OnceCell;
use validator::Validate;

use super::deployment::TargetNode;
use super::{
    Flake, MetaConfig, NixCommand, NixExpression, NixFlags, NodeConfig, NodeFilter, NodeName,
    ProfileDerivation, SerializedNixExpression, StorePath,
};
use crate::error::ColmenaResult;
use crate::job::JobHandle;
use crate::util::{CommandExecution, CommandExt};
use assets::Assets;

/// The version of the Hive schema we are compatible with.
///
/// Currently we are tied to one specific version.
const HIVE_SCHEMA: &str = "v0.5";

/// The snippet to be used for `nix eval --apply`.
const FLAKE_APPLY_SNIPPET: &str = formatcp!(
    r#"with builtins; hive: assert (hive.__schema == "{}" || throw ''
    The colmenaHive output (schema ${{hive.__schema}}) isn't compatible with this version of Colmena.

    Hint: Use the same version of Colmena as in the Flake input.
''); "#,
    HIVE_SCHEMA
);

/// The snippet mapping a value to the paths of the derivations in it.
///
/// It follows `nix-instantiate`: the value and each list element throw
/// unless they are a derivation, a list or a set, an attribute is skipped
/// unless it is a derivation or a set with `recurseForDerivations = true`,
/// and an output other than `out` is printed as `<drvPath>!<outputName>`.
/// Unlike `nix-instantiate`, it keeps attributes whose names fall outside
/// `[A-Za-z_][A-Za-z0-9_+-]*`, such as a node named `web.example.com`,
/// and calls no function, so a function or a set with `__functor` that is
/// not a derivation throws at the top level or in a list. Forcing
/// `drvPath` writes the derivation to the store under `nix eval` and
/// under `nix-instantiate --eval --read-write-mode` alike.
const DRV_PATHS_SNIPPET: &str = r#"(let
  drvPath = v: v.drvPath + (if v.outputName or "out" != "out" then "!" + v.outputName else "");
  isDrv = v: (v.type or null) == "derivation";
  inSet = v:
    if isDrv v then [ (drvPath v) ]
    else if builtins.isAttrs v && v.recurseForDerivations or false then inAttrs v
    else [ ];
  inAttrs = v: builtins.concatMap (n: builtins.addErrorContext "while evaluating the attribute '${n}'" (inSet v.${n})) (builtins.attrNames v);
  inList = v: builtins.concatMap (e: if isDrv e then [ (drvPath e) ] else toDrvPaths e) v;
  toDrvPaths = v:
    if isDrv v then [ (drvPath v) ]
    else if builtins.isAttrs v && !(v ? __functor) then inAttrs v
    else if builtins.isList v then inList v
    else throw "The expression must evaluate to a derivation, or a list or attribute set of derivations";
in toDrvPaths)"#;

#[derive(Debug)]
pub enum HivePath {
    /// A Nix Flake.
    ///
    /// The flake must expose a top-level `colmenaHive` output.
    Flake(Flake),

    /// A regular .nix file
    Legacy(PathBuf),
}

impl HivePath {
    /// Resolves a path or a flake URI into a HivePath.
    pub async fn resolve(s: &str, flags: &NixFlags) -> ColmenaResult<Self> {
        // TODO: handle an escaped colon

        let path = std::path::PathBuf::from(s);

        if !path.exists() && s.contains(':') {
            let flake = Flake::from_uri(s, flags).await?;

            tracing::info!("Using flake: {}", flake.uri());

            Ok(Self::Flake(flake))
        } else {
            Self::from_path(path, flags).await
        }
    }
}

#[derive(Debug)]
pub struct Hive {
    /// Path to the hive.
    path: HivePath,

    /// Path to the context directory.
    ///
    /// Normally this is directory containing the "hive.nix"
    /// or "flake.nix".
    context_dir: Option<PathBuf>,

    /// Static files required to evaluate a `hive.nix`, kept until the hive
    /// is dropped. A flake needs none.
    _assets: Option<Assets>,

    /// The expression from which the evaluated hive can be used.
    base_expression: String,

    /// Nix flags from the command line.
    flags: NixFlags,

    meta_config: OnceCell<MetaConfig>,
}

struct NixInstantiate<'hive> {
    hive: &'hive Hive,
    expression: String,
}

/// An expression to evaluate the system profiles of selected nodes.
struct EvalSelectedExpression<'hive> {
    hive: &'hive Hive,
    nodes_expr: SerializedNixExpression,
}

impl HivePath {
    pub async fn from_path<P: AsRef<Path>>(path: P, flags: &NixFlags) -> ColmenaResult<Self> {
        let path = path.as_ref();

        if let Some(osstr) = path.file_name()
            && osstr == "flake.nix"
        {
            let parent = path.parent().unwrap();
            let flake = Flake::from_dir(parent, flags).await?;
            return Ok(Self::Flake(flake));
        }

        Ok(Self::Legacy(path.canonicalize()?))
    }

    fn is_flake(&self) -> bool {
        matches!(self, Self::Flake(_))
    }

    fn context_dir(&self) -> Option<PathBuf> {
        match self {
            Self::Legacy(p) => p.parent().map(|d| d.to_owned()),
            Self::Flake(flake) => flake.local_dir().map(|d| d.to_owned()),
        }
    }
}

impl Hive {
    pub fn new(path: HivePath, flags: NixFlags) -> ColmenaResult<Self> {
        let context_dir = path.context_dir();
        let (assets, base_expression) = match &path {
            HivePath::Legacy(hive_nix) => {
                let assets = Assets::new()?;
                let base_expression = assets.get_base_expression(hive_nix);
                (Some(assets), base_expression)
            }
            HivePath::Flake(_) => (None, FLAKE_APPLY_SNIPPET.to_string()),
        };

        Ok(Self {
            path,
            context_dir,
            _assets: assets,
            base_expression,
            flags,
            meta_config: OnceCell::new(),
        })
    }

    pub fn context_dir(&self) -> Option<&Path> {
        self.context_dir.as_ref().map(|p| p.as_ref())
    }

    pub async fn get_meta_config(&self) -> ColmenaResult<&MetaConfig> {
        self.meta_config
            .get_or_try_init(|| async {
                self.nix_instantiate("hive.metaConfig")
                    .eval()
                    .capture_json()
                    .await
            })
            .await
    }

    /// Returns the flags from the command line, without the evaluation
    /// additions of [`Hive::nix_flags`] and [`Hive::nix_flags_with_builders`].
    pub fn base_flags(&self) -> NixFlags {
        self.flags.clone()
    }

    /// Returns Nix flags for evaluating this Hive, with pure
    /// evaluation enabled when the hive is a flake.
    pub fn nix_flags(&self) -> NixFlags {
        let mut flags = self.base_flags();
        flags.set_pure_eval(self.path.is_flake());
        flags
    }

    /// Returns Nix flags to set for this Hive, with configured remote builders.
    pub async fn nix_flags_with_builders(&self) -> ColmenaResult<NixFlags> {
        let mut flags = self.nix_flags();

        // an explicit --nix-option builders overrides meta.machinesFile
        if let Some(machines_file) = &self.get_meta_config().await?.machines_file
            && !flags.has_option("builders")
        {
            flags.add_option("builders".to_string(), format!("@{}", machines_file));
        }

        Ok(flags)
    }

    /// Convenience wrapper to filter nodes for CLI actions.
    pub async fn select_nodes(
        &self,
        filter: Option<NodeFilter>,
        ssh_config: Option<PathBuf>,
        ssh_only: bool,
    ) -> ColmenaResult<HashMap<NodeName, TargetNode>> {
        let mut node_configs = None;

        tracing::info!("Enumerating nodes...");

        let all_nodes = self.node_names().await?;
        let selected_nodes = match filter {
            Some(filter) => {
                if filter.has_node_config_rules() {
                    tracing::debug!("Retrieving deployment info for all nodes...");

                    let all_node_configs = self.deployment_info().await?;
                    let filtered = filter
                        .filter_node_configs(all_node_configs.iter())
                        .into_iter()
                        .collect();

                    node_configs = Some(all_node_configs);

                    filtered
                } else {
                    filter.filter_node_names(&all_nodes)?.into_iter().collect()
                }
            }
            None => all_nodes.clone(),
        };

        let n_selected = selected_nodes.len();

        let mut node_configs = if let Some(configs) = node_configs {
            configs
        } else {
            tracing::debug!("Retrieving deployment info for selected nodes...");
            self.deployment_info_selected(&selected_nodes).await?
        };

        let mut targets = HashMap::new();
        let mut n_ssh = 0;

        let nix_flags = self.base_flags();

        for node in selected_nodes.into_iter() {
            let config = node_configs.remove(&node).unwrap();

            let host = config.to_ssh_host(nix_flags.clone()).map(|mut host| {
                n_ssh += 1;

                if let Some(ssh_config) = &ssh_config {
                    host.set_ssh_config(ssh_config.clone());
                }

                if self.is_flake() {
                    host.set_use_nix3_copy(true);
                }

                host.upcast()
            });
            let ssh_host = host.is_some();
            let target = TargetNode::new(node.clone(), host, config);

            if !ssh_only || ssh_host {
                targets.insert(node, target);
            }
        }

        let skipped = n_selected - n_ssh;

        if targets.is_empty() {
            if skipped != 0 {
                tracing::warn!("No hosts selected.");
            } else {
                tracing::warn!("No hosts selected ({} skipped).", skipped);
            }
        } else if targets.len() == all_nodes.len() {
            tracing::info!("Selected all {} nodes.", targets.len());
        } else if !ssh_only || skipped == 0 {
            tracing::info!(
                "Selected {} out of {} hosts.",
                targets.len(),
                all_nodes.len()
            );
        } else {
            tracing::info!(
                "Selected {} out of {} hosts ({} skipped).",
                targets.len(),
                all_nodes.len(),
                skipped
            );
        }

        Ok(targets)
    }

    /// Returns a list of all node names.
    pub async fn node_names(&self) -> ColmenaResult<Vec<NodeName>> {
        self.nix_instantiate("attrNames hive.nodes")
            .eval()
            .capture_json()
            .await
    }

    /// Retrieve deployment info for all nodes.
    pub async fn deployment_info(&self) -> ColmenaResult<HashMap<NodeName, NodeConfig>> {
        let configs: HashMap<NodeName, NodeConfig> = self
            .nix_instantiate("hive.deploymentConfig")
            .eval_with_builders()
            .await?
            .capture_json()
            .await?;

        for config in configs.values() {
            config.validate()?;
            for key in config.keys.values() {
                key.validate()?;
            }
        }
        Ok(configs)
    }

    /// Retrieve deployment info for a single node.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub async fn deployment_info_single(
        &self,
        node: &NodeName,
    ) -> ColmenaResult<Option<NodeConfig>> {
        let expr = format!("hive.nodes.\"{}\".config.deployment or null", node.as_str());
        self.nix_instantiate(&expr)
            .eval_with_builders()
            .await?
            .capture_json()
            .await
    }

    /// Retrieve deployment info for a list of nodes.
    pub async fn deployment_info_selected(
        &self,
        nodes: &[NodeName],
    ) -> ColmenaResult<HashMap<NodeName, NodeConfig>> {
        let nodes_expr = SerializedNixExpression::new(nodes);

        let configs: HashMap<NodeName, NodeConfig> = self
            .nix_instantiate(&format!(
                "hive.deploymentConfigSelected {}",
                nodes_expr.expression()
            ))
            .eval_with_builders()
            .await?
            .capture_json()
            .await?;

        for config in configs.values() {
            config.validate()?;
            for key in config.keys.values() {
                key.validate()?;
            }
        }

        Ok(configs)
    }

    /// Evaluates selected nodes.
    ///
    /// Evaluation may take up a lot of memory, so we make it possible
    /// to split up the evaluation process into chunks and run them
    /// concurrently with other processes (e.g., build and apply).
    pub async fn eval_selected(
        &self,
        nodes: &[NodeName],
        job: Option<JobHandle>,
    ) -> ColmenaResult<HashMap<NodeName, ProfileDerivation>> {
        let nodes_expr = SerializedNixExpression::new(nodes);

        let expr = format!("hive.evalSelectedDrvPaths {}", nodes_expr.expression());

        let command = self.nix_instantiate(&expr).eval_with_builders().await?;
        let mut execution = CommandExecution::new(command);
        execution.set_job(job);
        execution.set_hide_stdout(true);

        execution
            .capture_json::<HashMap<NodeName, StorePath>>()
            .await?
            .into_iter()
            .map(|(name, path)| {
                let path = path.into_derivation()?;
                Ok((name, path))
            })
            .collect()
    }

    /// Returns the expression to evaluate selected nodes.
    pub fn eval_selected_expr(
        &self,
        nodes: &[NodeName],
    ) -> ColmenaResult<impl NixExpression + '_ + use<'_>> {
        let nodes_expr = SerializedNixExpression::new(nodes);

        Ok(EvalSelectedExpression {
            hive: self,
            nodes_expr,
        })
    }

    /// Evaluates an expression using values from the configuration.
    pub async fn introspect(&self, expression: String, instantiate: bool) -> ColmenaResult<String> {
        if instantiate {
            // the eval command of both hive kinds instantiates the
            // derivations whose drvPath it forces
            let expression = format!("{} (hive.introspect ({}))", DRV_PATHS_SNIPPET, expression);
            let paths: Vec<String> = self
                .nix_instantiate(&expression)
                .eval_with_builders()
                .await?
                .capture_json()
                .await?;

            // one path per line as nix-instantiate prints them, each path
            // once, whereas nix-instantiate skips only a value it has printed
            Ok(paths
                .into_iter()
                .unique()
                .map(|path| format!("{}\n", path))
                .collect())
        } else {
            let expression = format!("toJSON (hive.introspect ({}))", expression);
            self.nix_instantiate(&expression)
                .eval_with_builders()
                .await?
                .capture_json()
                .await
        }
    }

    /// Returns the expression for a REPL session.
    pub fn get_repl_expression(&self) -> String {
        let expression = format!("{} hive.introspect (x: x)", self.get_base_expression());

        match self.path() {
            HivePath::Legacy(_) => expression,
            // the base expression is a lambda for nix eval --apply, which means
            // nix repl needs it applied to the colmenaHive output here, from the
            // unlocked uri that nix eval reads too, so :reload refetches the flake,
            // though nix 2.26 to 2.34 cache git work tree status until the repl exits
            HivePath::Flake(flake) => format!(
                "({}) (builtins.getFlake \"{}\").outputs.colmenaHive",
                expression,
                flake.uri()
            ),
        }
    }

    /// Returns the base expression from which the evaluated Hive can be used.
    ///
    /// A `hive.nix` is evaluated by the bundled assets. A flake carries its
    /// evaluated hive in the `colmenaHive` output, which the expression
    /// receives as its argument from `nix eval --apply`,
    /// `nix-eval-jobs --select` or the repl expression.
    fn get_base_expression(&self) -> String {
        self.base_expression.clone()
    }

    /// Returns whether this Hive is a flake.
    fn is_flake(&self) -> bool {
        matches!(self.path(), HivePath::Flake(_))
    }

    fn nix_instantiate(&self, expression: &str) -> NixInstantiate<'_> {
        NixInstantiate::new(self, expression.to_owned())
    }

    fn path(&self) -> &HivePath {
        &self.path
    }
}

impl<'hive> NixInstantiate<'hive> {
    fn new(hive: &'hive Hive, expression: String) -> Self {
        Self { hive, expression }
    }

    fn eval_command(&self, flags: NixFlags) -> NixCommand {
        let mut full_expression = self.hive.get_base_expression();
        full_expression += &self.expression;

        match self.hive.path() {
            HivePath::Legacy(_) => NixCommand::nix_instantiate(flags)
                .arg("-E")
                .arg(full_expression)
                .args(["--eval", "--json", "--strict"])
                // --read-write-mode instantiates the derivations needed
                // for the system profile, IFD and eval --instantiate
                .arg("--read-write-mode"),
            HivePath::Flake(flake) => NixCommand::nix(flags)
                .arg("eval") // nix eval
                .arg(flake_installable(flake))
                .args(["--json", "--apply"])
                .arg(full_expression),
        }
    }

    fn eval(self) -> Command {
        let flags = self.hive.nix_flags();
        self.eval_command(flags).build()
    }

    async fn eval_with_builders(self) -> ColmenaResult<Command> {
        let flags = self.hive.nix_flags_with_builders().await?;
        Ok(self.eval_command(flags).build())
    }
}

impl NixExpression for EvalSelectedExpression<'_> {
    fn expression(&self) -> String {
        format!(
            "{} hive.evalSelected {}",
            self.hive.get_base_expression(),
            self.nodes_expr.expression(),
        )
    }

    fn installable(&self) -> Option<String> {
        match self.hive.path() {
            HivePath::Legacy(_) => None,
            HivePath::Flake(flake) => Some(flake_installable(flake)),
        }
    }
}

/// Returns the full `colmenaHive` accessor of a flake.
fn flake_installable(flake: &Flake) -> String {
    format!("{}#colmenaHive", flake.uri())
}
