use std::collections::HashMap;
use std::convert::TryInto;
use std::process::Stdio;

use tokio::process::Command;

use super::{key_uploader, CopyDirection, CopyOptions, Host};
use crate::error::{ColmenaError, ColmenaResult};
use crate::job::JobHandle;
use crate::nix::{Goal, Key, NixFlags, Profile, StorePath, CURRENT_PROFILE, SYSTEM_PROFILE};
use crate::util::{CommandExecution, CommandExt};

/// The local machine running Colmena.
///
/// It may not be capable of realizing some derivations
/// (e.g., building Linux derivations on macOS).
#[derive(Debug)]
pub struct Local {
    job: Option<JobHandle>,
    nix_options: NixFlags,
    privilege_escalation_command: Option<Vec<String>>,
}

impl Local {
    pub fn new(nix_options: NixFlags) -> Self {
        Self {
            job: None,
            nix_options,
            privilege_escalation_command: None,
        }
    }
}

impl Host for Local {
    async fn copy_closure(
        &mut self,
        _closure: &StorePath,
        _direction: CopyDirection,
        _options: CopyOptions,
    ) -> ColmenaResult<()> {
        Ok(())
    }

    async fn realize_remote(&mut self, derivation: &StorePath) -> ColmenaResult<Vec<StorePath>> {
        let mut command = Command::new("nix-store");

        command.args(self.nix_options.to_nix_store_args());
        command
            .arg("--no-gc-warning")
            .arg("--realise")
            .arg(derivation.as_path());

        let mut execution = CommandExecution::new(command);

        execution.set_job(self.job.clone());

        execution.run().await?;
        let (stdout, _) = execution.get_logs();

        stdout
            .unwrap()
            .lines()
            .map(|p| p.to_string().try_into())
            .collect()
    }

    async fn upload_keys(
        &mut self,
        keys: &HashMap<String, Key>,
        require_ownership: bool,
    ) -> ColmenaResult<()> {
        for (name, key) in keys {
            self.upload_key(name, key, require_ownership).await?;
        }

        Ok(())
    }

    async fn activate(&mut self, profile: &Profile, goal: Goal) -> ColmenaResult<()> {
        if !goal.requires_activation() {
            return Err(ColmenaError::Unsupported);
        }

        if goal.should_switch_profile() {
            let path = profile.as_path().to_str().unwrap();
            self.make_privileged_command(&["nix-env", "--profile", SYSTEM_PROFILE, "--set", path])
                .passthrough()
                .await?;
        }

        let command = {
            let activation_command = profile.activation_command(goal).unwrap();
            self.make_privileged_command(&activation_command)
        };

        let mut execution = CommandExecution::new(command);

        execution.set_job(self.job.clone());

        execution.run().await
    }

    async fn get_current_system_profile(&mut self) -> ColmenaResult<Profile> {
        let paths = Command::new("readlink")
            .args(["-e", CURRENT_PROFILE])
            .capture_output()
            .await?;

        let path = paths
            .lines()
            .next()
            .ok_or(ColmenaError::FailedToGetCurrentProfile)?
            .to_string()
            .try_into()?;

        Ok(Profile::from_store_path_unchecked(path))
    }

    async fn get_main_system_profile(&mut self) -> ColmenaResult<Profile> {
        let paths = Command::new("sh")
            .args([
                "-c",
                &format!(
                    "readlink -e {} || readlink -e {}",
                    SYSTEM_PROFILE, CURRENT_PROFILE
                ),
            ])
            .capture_output()
            .await?;

        let path = paths
            .lines()
            .next()
            .ok_or(ColmenaError::FailedToGetCurrentProfile)?
            .to_string()
            .try_into()?;

        Ok(Profile::from_store_path_unchecked(path))
    }

    fn set_job(&mut self, job: Option<JobHandle>) {
        self.job = job;
    }
}

impl Local {
    pub fn set_privilege_escalation_command(&mut self, command: Option<Vec<String>>) {
        self.privilege_escalation_command = command;
    }

    /// "Uploads" a single key.
    async fn upload_key(
        &mut self,
        name: &str,
        key: &Key,
        require_ownership: bool,
    ) -> ColmenaResult<()> {
        if let Some(job) = &self.job {
            job.message(format!("Deploying key {}", name))?;
        }

        let path = key.path();
        let key_script = format!(
            "'{}'",
            key_uploader::generate_script(key, path, require_ownership)
        );

        let mut command = self.make_privileged_command(&["sh", "-c", &key_script]);
        command.stdin(Stdio::piped());
        command.stderr(Stdio::piped());
        command.stdout(Stdio::piped());

        let uploader = command.spawn()?;
        key_uploader::feed_uploader(uploader, key, self.job.clone()).await
    }

    /// Constructs a command with privilege escalation.
    fn make_privileged_command<S: AsRef<str>>(&self, command: &[S]) -> Command {
        let mut full_command = Vec::new();
        if let Some(esc) = &self.privilege_escalation_command {
            full_command.extend(esc.iter().map(|s| s.as_str()));
        }
        full_command.extend(command.iter().map(|s| s.as_ref()));

        let mut result = Command::new(full_command[0]);
        if full_command.len() > 1 {
            result.args(&full_command[1..]);
        }

        result
    }
}
