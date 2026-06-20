use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;
use futures::future::join_all;
use tokio::sync::Semaphore;

use crate::error::ColmenaError;
use crate::job::{JobMonitor, JobState, JobType};
use crate::nix::host::Host;
use crate::nix::node_filter::NodeFilterOpts;
use crate::nix::Hive;
use crate::progress::SimpleProgressOutput;
use crate::util;

/// Run a command on remote machines
#[derive(Debug, Args)]
#[command(name = "exec")]
pub struct Opts {
    /// Deploy parallelism limit
    ///
    /// Limits the maximum number of hosts to run the command in parallel.
    ///
    /// In `colmena exec`, the parallelism limit is disabled (0) by default.
    #[arg(short, long, default_value_t = 0, value_name = "LIMIT")]
    parallel: usize,

    /// Be verbose
    ///
    /// Deactivates the progress spinner and prints every line of output.
    #[arg(short, long)]
    verbose: bool,

    #[command(flatten)]
    nodes: NodeFilterOpts,

    /// Command to run
    ///
    /// It's recommended to use -- to separate Colmena options from the command to run. For
    /// example:
    ///
    /// colmena exec --on @routers -- tcpdump -vni any ip[9] == 89
    #[arg(trailing_var_arg = true, required = true, value_name = "COMMAND")]
    command: Vec<String>,
}

pub async fn run(
    hive: Hive,
    Opts {
        parallel,
        verbose,
        nodes,
        command,
    }: Opts,
) -> Result<(), ColmenaError> {
    let ssh_config = env::var("SSH_CONFIG_FILE").ok().map(PathBuf::from);

    let mut targets = hive.select_nodes(nodes.on, ssh_config, true).await?;

    let parallel_sp = Arc::new(if parallel > 0 {
        Some(Semaphore::new(parallel))
    } else {
        None
    });

    let command = Arc::new(command);

    let mut output = SimpleProgressOutput::new(verbose);

    let (mut monitor, meta) = JobMonitor::new(output.get_sender());

    if let Some(width) = util::get_label_width(&targets) {
        monitor.set_label_width(width);
    }

    let meta = meta.run(|meta| async move {
        let mut futures = Vec::new();

        for (name, target) in targets.drain() {
            let parallel_sp = parallel_sp.clone();
            let command = Arc::clone(&command);

            let mut host = target.into_host().unwrap();

            let job = meta.create_job(JobType::Execute, vec![name.clone()])?;

            futures.push(job.run_waiting(|job| async move {
                let permit = match parallel_sp.as_ref() {
                    Some(sp) => Some(sp.acquire().await.unwrap()),
                    None => None,
                };

                job.state(JobState::Running)?;

                let command_v: Vec<&str> = command.iter().map(|s| s.as_str()).collect();
                host.set_job(Some(job));
                host.run_command(&command_v).await?;

                drop(permit);

                Ok(())
            }));
        }

        let results: Vec<Result<(), ColmenaError>> = join_all(futures).await;

        let mut failed: usize = 0;

        for x in results {
            match x {
                Err(_) => failed += 1,
                Ok(_) => (),
            }
        }

        Ok(failed)
    });

    let (meta, monitor, output) = tokio::join!(
        meta,
        monitor.run_until_completion(),
        output.run_until_completion(),
    );

    let failed = meta?;
    monitor?;
    output?;

    if failed > 0 {
        Err(ColmenaError::ExecError { n_hosts: failed })
    } else {
        Ok(())
    }
}
