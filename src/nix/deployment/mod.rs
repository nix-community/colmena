//! Deployment logic.

pub mod goal;

pub use goal::Goal;

pub mod limits;
pub use limits::{EvaluationNodeLimit, ParallelismLimit};

pub mod options;
pub use options::{EvaluatorType, Options};

use std::collections::HashMap;
use std::mem;
use std::sync::Arc;

use futures::future::join_all;
use itertools::Itertools;
use tokio_stream::StreamExt;

use super::host::AnyHost;
use super::NixFlags;
use crate::job::{JobHandle, JobMonitor, JobState, JobType};
use crate::progress::Sender as ProgressSender;
use crate::util;

use super::{
    evaluator::{DrvSetEvaluator, EvalError, NixEvalJobs},
    host::Local as LocalHost,
    key::{Key, UploadAt as UploadKeyAt},
    ColmenaError, ColmenaResult, CopyDirection, CopyOptions, Hive, Host, NodeConfig, NodeName,
    Profile, ProfileDerivation, RebootOptions,
};

/// A deployment.
pub type DeploymentHandle = Arc<Deployment>;

/// A map of target nodes.
pub type TargetNodeMap = HashMap<NodeName, TargetNode>;

/// A deployment.
#[derive(Debug)]
pub struct Deployment {
    /// The configuration.
    hive: Hive,

    /// The goal of this deployment.
    goal: Goal,

    /// Deployment options.
    options: Options,

    /// Options passed to Nix invocations.
    nix_options: NixFlags,

    /// Handle to send messages to the ProgressOutput.
    progress: Option<ProgressSender>,

    /// Handles to the deployment targets.
    targets: HashMap<NodeName, TargetNode>,

    /// Parallelism limit.
    parallelism_limit: ParallelismLimit,

    /// Evaluation limit.
    evaluation_node_limit: EvaluationNodeLimit,

    /// Whether it was executed.
    executed: bool,
}

/// Handle to a target node.
#[derive(Debug)]
pub struct TargetNode {
    /// Name of the node.
    name: NodeName,

    /// The host to deploy to.
    host: Option<AnyHost>,

    /// The config.deployment values of the node.
    config: NodeConfig,
}

impl TargetNode {
    pub fn new(name: NodeName, host: Option<AnyHost>, config: NodeConfig) -> Self {
        Self { name, host, config }
    }

    pub fn into_host(self) -> Option<AnyHost> {
        self.host
    }
}

impl Deployment {
    /// Creates a new deployment.
    pub fn new(
        hive: Hive,
        targets: TargetNodeMap,
        goal: Goal,
        progress: Option<ProgressSender>,
    ) -> Self {
        Self {
            hive,
            goal,
            options: Options::default(),
            nix_options: NixFlags::default(),
            progress,
            targets,
            parallelism_limit: ParallelismLimit::default(),
            evaluation_node_limit: EvaluationNodeLimit::default(),
            executed: false,
        }
    }

    /// Executes the deployment.
    ///
    /// If a ProgressSender is supplied, then this should be run in parallel
    /// with its `run_until_completion()` future.
    pub async fn execute(mut self) -> ColmenaResult<()> {
        if self.executed {
            return Err(ColmenaError::DeploymentAlreadyExecuted);
        }

        self.executed = true;

        let (mut monitor, meta) = JobMonitor::new(self.progress.clone());

        if let Some(width) = util::get_label_width(&self.targets) {
            monitor.set_label_width(width);
        }

        let nix_options = self.hive.nix_flags_with_builders().await?;
        self.nix_options = nix_options;

        if self.goal == Goal::UploadKeys {
            // Just upload keys
            let targets = mem::take(&mut self.targets);
            let deployment = DeploymentHandle::new(self);
            let meta_future = meta.run(|meta| async move {
                let mut futures = Vec::new();

                for target in targets.into_values() {
                    futures.push(deployment.upload_keys_to_node(meta.clone(), target));
                }

                join_all(futures)
                    .await
                    .into_iter()
                    .collect::<ColmenaResult<Vec<()>>>()?;

                Ok(())
            });

            let (result, _) = tokio::join!(meta_future, monitor.run_until_completion(),);

            result?;

            Ok(())
        } else {
            // Do the whole eval-build-deploy flow
            let targets = mem::take(&mut self.targets);
            let deployment = DeploymentHandle::new(self);
            let meta_future = meta.run(|meta| async move {
                match deployment.options.evaluator {
                    EvaluatorType::Chunked => {
                        deployment.execute_chunked(meta.clone(), targets).await?;
                    }
                    EvaluatorType::Streaming => {
                        tracing::warn!("Streaming evaluation is an experimental feature");
                        deployment.execute_streaming(meta.clone(), targets).await?;
                    }
                }

                Ok(())
            });

            let (result, _) = tokio::join!(meta_future, monitor.run_until_completion(),);

            result?;

            Ok(())
        }
    }

    pub fn set_options(&mut self, options: Options) {
        self.options = options;
    }

    pub fn set_parallelism_limit(&mut self, limit: ParallelismLimit) {
        self.parallelism_limit = limit;
    }

    pub fn set_evaluation_node_limit(&mut self, limit: EvaluationNodeLimit) {
        self.evaluation_node_limit = limit;
    }

    /// Executes the deployment on selected nodes, evaluating a chunk at a time.
    async fn execute_chunked(
        self: &DeploymentHandle,
        parent: JobHandle,
        mut targets: TargetNodeMap,
    ) -> ColmenaResult<()> {
        let eval_limit = self
            .evaluation_node_limit
            .get_limit()
            .unwrap_or(targets.len());

        let mut futures = Vec::new();

        for chunk in targets.drain().chunks(eval_limit).into_iter() {
            let mut map = HashMap::new();
            for (name, host) in chunk {
                map.insert(name, host);
            }

            futures.push(self.execute_one_chunk(parent.clone(), map));
        }

        join_all(futures)
            .await
            .into_iter()
            .collect::<ColmenaResult<Vec<()>>>()?;

        Ok(())
    }

    /// Executes the deployment on selected nodes using a streaming evaluator.
    async fn execute_streaming(
        self: &DeploymentHandle,
        parent: JobHandle,
        mut targets: TargetNodeMap,
    ) -> ColmenaResult<()> {
        if self.goal == Goal::UploadKeys {
            unreachable!(); // some logic is screwed up
        }

        let nodes: Vec<NodeName> = targets.keys().cloned().collect();
        let expr = self.hive.eval_selected_expr(&nodes)?;

        let job = parent.create_job(JobType::Evaluate, nodes.clone())?;

        let (futures, failed_attributes) = job
            .run(|job| async move {
                let mut evaluator = NixEvalJobs::default();
                let eval_limit = self
                    .evaluation_node_limit
                    .get_limit()
                    .unwrap_or(self.targets.len());
                evaluator.set_eval_limit(eval_limit);
                evaluator.set_job(job.clone());

                // FIXME: nix-eval-jobs currently does not support IFD with builders
                let options = self.hive.nix_flags();
                let mut stream = evaluator.evaluate(&expr, options).await?;

                let mut futures: Vec<tokio::task::JoinHandle<ColmenaResult<()>>> = Vec::new();
                let mut failed_attributes = Vec::new();

                while let Some(item) = stream.next().await {
                    match item {
                        Ok(attr) => {
                            let node_name = NodeName::new(attr.attribute().to_owned())?;
                            let profile_drv: ProfileDerivation = attr.into_derivation()?;

                            // FIXME: Consolidate
                            let mut target = targets.remove(&node_name).unwrap();

                            if let Some(force_build_on_target) = self.options.force_build_on_target
                            {
                                target.config.set_build_on_target(force_build_on_target);
                            }

                            let job_handle = job.clone();
                            let arc_self = self.clone();
                            futures.push(tokio::spawn(async move {
                                let (target, profile) = {
                                    if target.config.build_on_target() {
                                        arc_self
                                            .build_on_node(
                                                job_handle.clone(),
                                                target,
                                                profile_drv.clone(),
                                            )
                                            .await?
                                    } else {
                                        arc_self
                                            .build_and_push_node(
                                                job_handle.clone(),
                                                target,
                                                profile_drv.clone(),
                                            )
                                            .await?
                                    }
                                };

                                if arc_self.goal.requires_activation() {
                                    arc_self.activate_node(job_handle, target, profile).await
                                } else {
                                    Ok(())
                                }
                            }));
                        }
                        Err(e) => {
                            match e {
                                EvalError::Global(e) => {
                                    // Global error - Abort immediately
                                    return Err(e);
                                }
                                EvalError::Attribute(e) => {
                                    // Attribute-level error
                                    //
                                    // We still let the rest of the evaluation finish but
                                    // mark the whole Evaluate job as failed.

                                    let node_name =
                                        NodeName::new(e.attribute().to_string()).unwrap();
                                    let nodes = vec![node_name.clone()];
                                    let job = parent.create_job(JobType::Evaluate, nodes)?;

                                    job.state(JobState::Running)?;
                                    for line in e.error().lines() {
                                        job.stderr(line.to_string())?;
                                    }
                                    job.state(JobState::Failed)?;

                                    failed_attributes.push(node_name);
                                }
                            }
                        }
                    }
                }

                // HACK: Still return Ok() because we need to wait for existing jobs to finish
                if !failed_attributes.is_empty() {
                    job.failure(&ColmenaError::AttributeEvaluationError)?;
                }

                Ok((futures, failed_attributes))
            })
            .await?;

        join_all(futures)
            .await
            .into_iter()
            .map(|r| r.unwrap()) // panic on JoinError (future panicked)
            .collect::<ColmenaResult<Vec<()>>>()?;

        if !failed_attributes.is_empty() {
            Err(ColmenaError::AttributeEvaluationError)
        } else {
            Ok(())
        }
    }

    /// Executes the deployment against a portion of nodes.
    async fn execute_one_chunk(
        self: &DeploymentHandle,
        parent: JobHandle,
        mut chunk: TargetNodeMap,
    ) -> ColmenaResult<()> {
        if self.goal == Goal::UploadKeys {
            unreachable!(); // some logic is screwed up
        }

        let nodes: Vec<NodeName> = chunk.keys().cloned().collect();
        let profile_drvs = self.evaluate_nodes(parent.clone(), nodes.clone()).await?;

        let mut futures = Vec::new();

        for (name, profile_drv) in profile_drvs.iter() {
            let mut target = chunk.remove(name).unwrap();

            if let Some(force_build_on_target) = self.options.force_build_on_target {
                target.config.set_build_on_target(force_build_on_target);
            }

            let job_handle = parent.clone();
            let arc_self = self.clone();
            futures.push(async move {
                let (target, profile) = {
                    if target.config.build_on_target() {
                        arc_self
                            .build_on_node(job_handle.clone(), target, profile_drv.clone())
                            .await?
                    } else {
                        arc_self
                            .build_and_push_node(job_handle.clone(), target, profile_drv.clone())
                            .await?
                    }
                };

                if arc_self.goal.requires_activation() {
                    arc_self.activate_node(job_handle, target, profile).await
                } else {
                    Ok(())
                }
            });
        }

        join_all(futures)
            .await
            .into_iter()
            .collect::<ColmenaResult<Vec<()>>>()?;

        Ok(())
    }

    /// Evaluates a set of nodes, returning their corresponding store derivations.
    async fn evaluate_nodes(
        self: &DeploymentHandle,
        parent: JobHandle,
        nodes: Vec<NodeName>,
    ) -> ColmenaResult<HashMap<NodeName, ProfileDerivation>> {
        let job = parent.create_job(JobType::Evaluate, nodes.clone())?;

        job.run_waiting(|job| async move {
            // Wait for eval limit
            let permit = self.parallelism_limit.evaluation.acquire().await.unwrap();
            job.state(JobState::Running)?;

            let result = self.hive.eval_selected(&nodes, Some(job.clone())).await;

            drop(permit);
            result
        })
        .await
    }

    /// Only uploads keys to a node.
    async fn upload_keys_to_node(
        self: &DeploymentHandle,
        parent: JobHandle,
        mut target: TargetNode,
    ) -> ColmenaResult<()> {
        let nodes = vec![target.name.clone()];
        let job = parent.create_job(JobType::UploadKeys, nodes)?;
        job.run(|job| async move {
            if target.host.is_none() {
                return Err(ColmenaError::Unsupported);
            }

            let host = target.host.as_mut().unwrap();
            host.set_job(Some(job));
            host.upload_keys(&target.config.keys, true).await?;

            Ok(())
        })
        .await
    }

    /// Builds a system profile directly on the node itself.
    async fn build_on_node(
        self: &DeploymentHandle,
        parent: JobHandle,
        mut target: TargetNode,
        profile_drv: ProfileDerivation,
    ) -> ColmenaResult<(TargetNode, Profile)> {
        let nodes = vec![target.name.clone()];

        let permit = self.parallelism_limit.apply.acquire().await.unwrap();

        let build_job = parent.create_job(JobType::Build, nodes.clone())?;
        let (target, profile) = build_job
            .run(|job| async move {
                if target.host.is_none() {
                    return Err(ColmenaError::Unsupported);
                }

                let host = target.host.as_mut().unwrap();
                host.set_job(Some(job.clone()));

                host.copy_closure(
                    profile_drv.as_store_path(),
                    CopyDirection::ToRemote,
                    CopyOptions::default().include_outputs(true),
                )
                .await?;

                let profile = profile_drv.realize_remote(host).await?;

                job.success_with_message(format!("Built {:?} on target node", profile.as_path()))?;
                Ok((target, profile))
            })
            .await?;

        drop(permit);

        Ok((target, profile))
    }

    /// Builds and pushes a system profile on a node.
    async fn build_and_push_node(
        self: &DeploymentHandle,
        parent: JobHandle,
        target: TargetNode,
        profile_drv: ProfileDerivation,
    ) -> ColmenaResult<(TargetNode, Profile)> {
        let nodes = vec![target.name.clone()];

        let permit = self.parallelism_limit.apply.acquire().await.unwrap();

        // Build system profile
        let build_job = parent.create_job(JobType::Build, nodes.clone())?;
        let arc_self = self.clone();
        let profile: Profile = build_job
            .run(|job| async move {
                // FIXME: Remote builder?
                let mut builder: AnyHost = LocalHost::new(arc_self.nix_options.clone()).into();
                builder.set_job(Some(job.clone()));

                let profile = profile_drv.realize(&mut builder).await?;

                job.success_with_message(format!("Built {:?}", profile.as_path()))?;
                Ok(profile)
            })
            .await?;

        // Create GC root
        let profile_r = profile.clone();
        let mut target = if self.options.create_gc_roots {
            let job = parent.create_job(JobType::CreateGcRoots, nodes.clone())?;
            let arc_self = self.clone();
            job.run_waiting(|job| async move {
                if let Some(dir) = arc_self.hive.context_dir() {
                    job.state(JobState::Running)?;
                    let path = dir.join(".gcroots").join(format!("node-{}", &*target.name));

                    profile_r.create_gc_root(&path).await?;
                } else {
                    job.noop("No context directory to create GC roots in".to_string())?;
                }
                Ok(target)
            })
            .await?
        } else {
            target
        };

        if self.goal == Goal::Build {
            return Ok((target, profile));
        }

        // Push closure to remote
        let push_job = parent.create_job(JobType::Push, nodes.clone())?;
        let push_profile = profile.clone();
        let arc_self = self.clone();
        let target = push_job
            .run(|job| async move {
                if target.host.is_none() {
                    return Err(ColmenaError::Unsupported);
                }

                let host = target.host.as_mut().unwrap();
                host.set_job(Some(job.clone()));
                host.copy_closure(
                    push_profile.as_store_path(),
                    CopyDirection::ToRemote,
                    arc_self.options.to_copy_options(),
                )
                .await?;

                Ok(target)
            })
            .await?;

        drop(permit);

        Ok((target, profile))
    }

    /// Activates a system profile on a node.
    ///
    /// This will also upload keys to the node.
    async fn activate_node(
        self: DeploymentHandle,
        parent: JobHandle,
        mut target: TargetNode,
        profile: Profile,
    ) -> ColmenaResult<()> {
        let nodes = vec![target.name.clone()];

        let permit = self.parallelism_limit.apply.acquire().await.unwrap();

        // Upload pre-activation keys
        let mut target = if self.options.upload_keys {
            let job = parent.create_job(JobType::UploadKeys, nodes.clone())?;
            job.run_waiting(|job| async move {
                let keys = target
                    .config
                    .keys
                    .iter()
                    .filter(|(_, v)| v.upload_at() == UploadKeyAt::PreActivation)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<HashMap<String, Key>>();

                if keys.is_empty() {
                    job.noop("No pre-activation keys to upload".to_string())?;
                    return Ok(target);
                }

                job.state(JobState::Running)?;
                job.message("Uploading pre-activation keys...".to_string())?;

                let host = target.host.as_mut().unwrap();
                host.set_job(Some(job.clone()));
                host.upload_keys(&keys, false).await?;

                job.success_with_message("Uploaded keys (pre-activation)".to_string())?;
                Ok(target)
            })
            .await?
        } else {
            target
        };

        // Activate profile
        let activation_job = parent.create_job(JobType::Activate, nodes.clone())?;
        let arc_self = self.clone();
        let profile_r = profile.clone();
        let mut target = activation_job.run(|job| async move {
            let host = target.host.as_mut().unwrap();
            host.set_job(Some(job.clone()));

            if !target.config.replace_unknown_profiles {
                job.message("Checking remote profile...".to_string())?;

                let profile = host.get_main_system_profile().await?;

                if profile.as_store_path().exists() {
                    job.message("Remote profile known".to_string())?;
                } else if arc_self.options.force_replace_unknown_profiles {
                    job.message("Warning: Remote profile is unknown, but unknown profiles are being ignored".to_string())?;
                } else {
                    return Err(ColmenaError::ActiveProfileUnknown {
                        profile,
                    });
                }
            }

            host.activate(&profile_r, arc_self.goal).await?;

            job.success_with_message(arc_self.goal.success_str().to_string())?;

            Ok(target)
        }).await?;

        // Reboot
        let mut target = if self.options.reboot {
            let job = parent.create_job(JobType::Reboot, nodes.clone())?;
            let arc_self = self.clone();
            job.run(|job| async move {
                let host = target.host.as_mut().unwrap();
                host.set_job(Some(job.clone()));

                let new_profile = if arc_self.goal.persists_after_reboot() {
                    Some(profile)
                } else {
                    None
                };

                let options = RebootOptions::default()
                    .wait_for_boot(true)
                    .new_profile(new_profile);

                host.reboot(options).await?;

                Ok(target)
            })
            .await?
        } else {
            target
        };

        // Upload post-activation keys
        if self.options.upload_keys {
            let job = parent.create_job(JobType::UploadKeys, nodes.clone())?;
            job.run_waiting(|job| async move {
                let keys = target
                    .config
                    .keys
                    .iter()
                    .filter(|(_, v)| v.upload_at() == UploadKeyAt::PostActivation)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<HashMap<String, Key>>();

                if keys.is_empty() {
                    job.noop("No post-activation keys to upload".to_string())?;
                    return Ok(());
                }

                job.state(JobState::Running)?;
                job.message("Uploading post-activation keys...".to_string())?;

                let host = target.host.as_mut().unwrap();
                host.set_job(Some(job.clone()));
                host.upload_keys(&keys, true).await?;

                job.success_with_message("Uploaded keys (post-activation)".to_string())?;
                Ok(())
            })
            .await?;
        }

        drop(permit);

        Ok(())
    }
}
