use std::collections::HashMap;
use std::convert::TryInto;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;

use super::{CopyDirection, CopyOptions, Host, key_uploader};
use crate::error::{ColmenaError, ColmenaResult};
use crate::job::JobHandle;
use crate::nix::{
    CURRENT_PROFILE, DARWIN_NIX_BIN_PATH, Goal, Key, NixFlags, Profile, SYSTEM_PROFILE, StorePath,
    SystemType,
};
use crate::util::{CommandExecution, CommandExt};

/// Resolves a nix binary name to an absolute path when running on macOS.
///
/// The [`Local`] host always executes on the machine running Colmena, so the
/// relevant OS is the compile target. On macOS, root's PATH (and sudo's
/// `secure_path`) excludes the nix profile bin dir, so we must use an absolute
/// path; on other platforms the bare name is resolved via PATH as usual.
fn local_nix_bin(name: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("{DARWIN_NIX_BIN_PATH}/{name}")
    } else {
        name.to_string()
    }
}

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

#[async_trait]
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
        let mut command = Command::new(local_nix_bin("nix-store"));

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

    async fn activate(
        &mut self,
        profile: &Profile,
        goal: Goal,
        system_type: SystemType,
    ) -> ColmenaResult<()> {
        if !goal.requires_activation() {
            return Err(ColmenaError::Unsupported);
        }

        // Check if this goal is supported for Darwin
        if system_type.is_darwin() && !goal.supported_on_darwin() {
            return Err(ColmenaError::Unsupported);
        }

        if goal.should_switch_profile() {
            let path = profile.as_path().to_str().unwrap();
            let nix_env = local_nix_bin("nix-env");
            self.make_privileged_command(&[
                nix_env.as_str(),
                "--profile",
                SYSTEM_PROFILE,
                "--set",
                path,
            ])
            .passthrough()
            .await?;
        }

        let command = {
            let activation_command = profile
                .activation_command(goal, system_type)
                .ok_or(ColmenaError::Unsupported)?;
            self.make_privileged_command(&activation_command)
        };

        let mut execution = CommandExecution::new(command);

        execution.set_job(self.job.clone());

        execution.run().await
    }

    async fn get_current_system_profile(&mut self) -> ColmenaResult<Profile> {
        // `readlink -f` (not GNU-only `-e`) so this also works on macOS/BSD.
        // CURRENT_PROFILE is always a valid symlink on a live system.
        let paths = Command::new("readlink")
            .args(["-f", CURRENT_PROFILE])
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
        // `readlink -f` for GNU/BSD portability. The `[ -e ]` guard preserves
        // the "final target must exist" semantics of the old `readlink -e`
        // fallback: a bare `-f` prints a non-existent path (exit 0) under GNU,
        // which would wrongly suppress the fallback to CURRENT_PROFILE.
        let paths = Command::new("sh")
            .args([
                "-c",
                &format!(
                    "if [ -e {sys} ]; then readlink -f {sys}; else readlink -f {cur}; fi",
                    sys = SYSTEM_PROFILE,
                    cur = CURRENT_PROFILE
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
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn set_privilege_escalation_command(&mut self, command: Option<Vec<String>>) {
        self.privilege_escalation_command = command;
    }

    pub fn upcast(self) -> Box<dyn Host> {
        Box::new(self)
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
