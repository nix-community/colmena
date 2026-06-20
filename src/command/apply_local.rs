use regex::Regex;
use std::collections::HashMap;

use clap::Args;
use tokio::fs;

use crate::error::ColmenaError;

use crate::nix::deployment::{Deployment, Goal, Options, TargetNode};
use crate::nix::Hive;
use crate::nix::{host::Local as LocalHost, NodeName};
use crate::progress::SimpleProgressOutput;

/// Apply configurations on the local machine
#[derive(Debug, Args)]
#[command(name = "apply-local")]
pub struct Opts {
    /// Deployment goal
    ///
    /// Same as the targets for switch-to-configuration.
    /// "push" is noop in apply-local.
    #[arg(value_name = "GOAL", default_value_t)]
    goal: Goal,

    /// Attempt to escalate privileges if not run as root
    #[arg(long)]
    sudo: bool,

    /// Be verbose
    ///
    /// Deactivates the progress spinner and prints every line of output.
    #[arg(short, long)]
    verbose: bool,

    /// Do not deploy keys
    ///
    /// Do not deploy secret keys set in `deployment.keys`. By default, Colmena will deploy keys
    /// set in `deployment.keys` before activating the profile on this host.
    #[arg(long)]
    no_keys: bool,

    /// Override the node name to use
    #[arg(long)]
    node: Option<String>,

    /// Removed: Configure deployment.privilegeEscalationCommand in node configuration
    #[arg(long, value_name = "COMMAND", hide = true)]
    sudo_command: Option<String>,
}

pub async fn run(
    hive: Hive,
    Opts {
        goal,
        sudo,
        verbose,
        no_keys,
        node,
        sudo_command,
    }: Opts,
) -> Result<(), ColmenaError> {
    if sudo_command.is_some() {
        tracing::error!("--sudo-command has been removed. Please configure it in deployment.privilegeEscalationCommand in the node configuration.");
        quit::with_code(1);
    }

    // Sanity check: Are we running NixOS?
    if let Ok(os_release) = fs::read_to_string("/etc/os-release").await {
        let re = Regex::new(r#"ID="?nixos"?"#).unwrap();
        if !re.is_match(&os_release) {
            tracing::error!("\"apply-local\" only works on NixOS machines.");
            quit::with_code(5);
        }
    } else {
        tracing::error!("Could not detect the OS version from /etc/os-release.");
        quit::with_code(5);
    }

    let verbose = verbose || sudo; // cannot use spinners with interactive sudo

    {
        let euid: u32 = unsafe { libc::geteuid() };
        if euid != 0 && !sudo {
            tracing::warn!("Colmena was not started by root. This is probably not going to work.");
            tracing::warn!("Hint: Add the --sudo flag.");
        }
    }

    let hostname = NodeName::new(node.unwrap_or_else(|| {
        hostname::get()
            .expect("Could not get hostname")
            .to_string_lossy()
            .into_owned()
    }))?;

    let target = {
        if let Some(info) = hive.deployment_info_single(&hostname).await.unwrap() {
            let nix_options = hive.nix_flags_with_builders().await.unwrap();
            if !info.allows_local_deployment() {
                tracing::error!(
                    "Local deployment is not enabled for host {}.",
                    hostname.as_str()
                );
                tracing::error!("Hint: Set deployment.allowLocalDeployment to true.");
                quit::with_code(2);
            }
            let mut host = LocalHost::new(nix_options);
            if sudo {
                let command = info.privilege_escalation_command().to_owned();
                host.set_privilege_escalation_command(Some(command));
            }

            TargetNode::new(hostname.clone(), Some(host.into()), info.clone())
        } else {
            tracing::error!(
                "Host \"{}\" is not present in the Hive configuration.",
                hostname.as_str()
            );
            quit::with_code(2);
        }
    };

    let mut targets = HashMap::new();
    targets.insert(hostname.clone(), target);

    let mut output = SimpleProgressOutput::new(verbose);
    let progress = output.get_sender();

    let mut deployment = Deployment::new(hive, targets, goal, progress);

    let options = {
        let mut options = Options::default();
        options.set_upload_keys(!no_keys);
        options
    };

    deployment.set_options(options);

    let (deployment, output) = tokio::join!(deployment.execute(), output.run_until_completion());

    deployment?;
    output?;

    Ok(())
}
