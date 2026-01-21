use std::collections::HashMap;
use std::convert::TryInto;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::time::sleep;

use super::{CopyDirection, CopyOptions, Host, RebootOptions, key_uploader};
use crate::error::{ColmenaError, ColmenaResult};
use crate::job::JobHandle;
use crate::nix::{CURRENT_PROFILE, Goal, Key, Profile, SYSTEM_PROFILE, StorePath, SystemType};
use crate::util::{CommandExecution, CommandExt};

/// Default nix bin directory on macOS.
/// Root's PATH on macOS doesn't include nix binaries by default
/// (/usr/bin:/bin:/usr/sbin:/sbin). This is the canonical path that
/// both /var/root/.nix-profile and /nix/var/nix/profiles/default symlink to.
const DARWIN_NIX_BIN_PATH: &str = "/nix/var/nix/profiles/default/bin";

/// A remote machine connected over SSH.
#[derive(Debug)]
pub struct Ssh {
    /// The username to use to connect.
    user: Option<String>,

    /// The hostname or IP address to connect to.
    host: String,

    /// The port to connect to.
    port: Option<u16>,

    /// Local path to a ssh_config file.
    ssh_config: Option<PathBuf>,

    /// Command to elevate privileges with.
    privilege_escalation_command: Vec<String>,

    /// extra SSH options
    extra_ssh_options: Vec<String>,

    /// Whether to use the experimental `nix copy` command.
    use_nix3_copy: bool,

    /// The system type (NixOS or Darwin).
    /// Used to handle platform-specific differences like nix-daemon path on macOS.
    system_type: SystemType,

    job: Option<JobHandle>,
}

/// An opaque boot ID.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BootId(String);

#[async_trait]
impl Host for Ssh {
    async fn copy_closure(
        &mut self,
        closure: &StorePath,
        direction: CopyDirection,
        options: CopyOptions,
    ) -> ColmenaResult<()> {
        let command = self.nix_copy_closure(closure, direction, options);
        self.run_command(command).await
    }

    async fn realize_remote(&mut self, derivation: &StorePath) -> ColmenaResult<Vec<StorePath>> {
        let command = self.ssh(&[
            "nix-store",
            "--no-gc-warning",
            "--realise",
            derivation.as_path().to_str().unwrap(),
        ]);

        let mut execution = CommandExecution::new(command);
        execution.set_job(self.job.clone());

        let paths = execution.capture_output().await?;

        paths.lines().map(|p| p.to_string().try_into()).collect()
    }

    fn set_job(&mut self, job: Option<JobHandle>) {
        self.job = job;
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
            // Use full path for nix-env on Darwin since root's PATH doesn't include nix binaries
            let nix_env = if system_type.is_darwin() {
                format!("{}/nix-env", DARWIN_NIX_BIN_PATH)
            } else {
                "nix-env".to_string()
            };
            let set_profile = self.ssh(&[&nix_env, "--profile", SYSTEM_PROFILE, "--set", path]);
            self.run_command(set_profile).await?;
        }

        // Get the activation command based on system type
        let activation_command = profile
            .activation_command(goal, system_type)
            .ok_or(ColmenaError::Unsupported)?;
        let v: Vec<&str> = activation_command.iter().map(|s| &**s).collect();
        let command = self.ssh(&v);
        self.run_command(command).await
    }

    async fn get_current_system_profile(&mut self) -> ColmenaResult<Profile> {
        let paths = self
            .ssh(&["readlink", "-e", CURRENT_PROFILE])
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
        let command = format!(
            "\"readlink -e {} || readlink -e {}\"",
            SYSTEM_PROFILE, CURRENT_PROFILE
        );

        let paths = self.ssh(&["sh", "-c", &command]).capture_output().await?;

        let path = paths
            .lines()
            .next()
            .ok_or(ColmenaError::FailedToGetCurrentProfile)?
            .to_string()
            .try_into()?;

        Ok(Profile::from_store_path_unchecked(path))
    }

    async fn run_command(&mut self, command: &[&str]) -> ColmenaResult<()> {
        let command = self.ssh(command);
        self.run_command(command).await
    }

    async fn reboot(&mut self, options: RebootOptions) -> ColmenaResult<()> {
        if !options.wait_for_boot {
            return self.initate_reboot().await;
        }

        let system_type = options.get_system_type();
        let old_id = self.get_boot_id(system_type).await?;

        self.initate_reboot().await?;

        if let Some(job) = &self.job {
            job.message("Waiting for reboot".to_string())?;
        }

        // Wait for node to come back up
        loop {
            // Ignore errors while waiting
            if let Ok(new_id) = self.get_boot_id(system_type).await
                && new_id != old_id
            {
                break;
            }

            sleep(Duration::from_secs(2)).await;
        }

        // Ensure node has correct system profile
        if let Some(new_profile) = options.new_profile {
            let profile = self.get_current_system_profile().await?;

            if new_profile != profile {
                return Err(ColmenaError::ActiveProfileUnexpected { profile });
            }
        }

        Ok(())
    }
}

impl Ssh {
    pub fn new(user: Option<String>, host: String) -> Self {
        Self {
            user,
            host,
            port: None,
            ssh_config: None,
            privilege_escalation_command: Vec::new(),
            extra_ssh_options: Vec::new(),
            use_nix3_copy: false,
            system_type: SystemType::default(),
            job: None,
        }
    }

    pub fn set_port(&mut self, port: u16) {
        self.port = Some(port);
    }

    pub fn set_ssh_config(&mut self, ssh_config: PathBuf) {
        self.ssh_config = Some(ssh_config);
    }

    pub fn set_privilege_escalation_command(&mut self, command: Vec<String>) {
        self.privilege_escalation_command = command;
    }

    pub fn set_extra_ssh_options(&mut self, options: Vec<String>) {
        self.extra_ssh_options = options;
    }

    pub fn set_use_nix3_copy(&mut self, enable: bool) {
        self.use_nix3_copy = enable;
    }

    pub fn set_system_type(&mut self, system_type: SystemType) {
        self.system_type = system_type;
    }

    pub fn upcast(self) -> Box<dyn Host> {
        Box::new(self)
    }

    /// Returns a Tokio Command to run an arbitrary command on the host.
    pub fn ssh(&self, command: &[&str]) -> Command {
        let options = self.ssh_options();
        let options_str = options.join(" ");
        let privilege_escalation_command = if self.user.as_deref() != Some("root") {
            self.privilege_escalation_command.as_slice()
        } else {
            &[]
        };

        let mut cmd = Command::new("ssh");

        cmd.arg(self.ssh_target())
            .args(&options)
            .arg("--")
            .args(privilege_escalation_command)
            .args(command)
            .env("NIX_SSHOPTS", options_str);

        cmd
    }

    async fn run_command(&mut self, command: Command) -> ColmenaResult<()> {
        let mut execution = CommandExecution::new(command);
        execution.set_job(self.job.clone());

        execution.run().await
    }

    fn ssh_target(&self) -> String {
        match &self.user {
            Some(n) => format!("{}@{}", n, self.host),
            None => self.host.clone(),
        }
    }

    fn nix_copy_closure(
        &self,
        path: &StorePath,
        direction: CopyDirection,
        options: CopyOptions,
    ) -> Command {
        let ssh_options = self.ssh_options();
        let ssh_options_str = ssh_options.join(" ");

        let mut command = if self.use_nix3_copy {
            // experimental `nix copy` command with ssh-ng://
            let mut command = Command::new("nix");

            command.args([
                "--extra-experimental-features",
                "nix-command",
                "copy",
                "--no-check-sigs",
            ]);

            if options.use_substitutes {
                command.args([
                    "--substitute-on-destination",
                    // needed due to UX bug in ssh-ng://
                    "--builders-use-substitutes",
                ]);
            }

            if let Some("drv") = path.extension().and_then(OsStr::to_str) {
                command.arg("--derivation");
            }

            match direction {
                CopyDirection::ToRemote => {
                    command.arg("--to");
                }
                CopyDirection::FromRemote => {
                    command.arg("--from");
                }
            }

            // Build the store URI with appropriate query parameters
            // For darwin, we need to specify the remote-program because root's PATH
            // on macOS doesn't include nix binaries by default
            let mut store_uri = format!("ssh-ng://{}", self.ssh_target());
            let mut query_params = Vec::new();

            if self.system_type.is_darwin() {
                query_params.push(format!("remote-program={}/nix-daemon", DARWIN_NIX_BIN_PATH));
            }

            if options.gzip {
                query_params.push("compress=true".to_string());
            }

            if !query_params.is_empty() {
                store_uri = format!("{}?{}", store_uri, query_params.join("&"));
            }

            command.arg(store_uri);

            command.arg(path.as_path());

            command
        } else {
            // nix-copy-closure (ssh://)
            let mut command = Command::new("nix-copy-closure");

            match direction {
                CopyDirection::ToRemote => {
                    command.arg("--to");
                }
                CopyDirection::FromRemote => {
                    command.arg("--from");
                }
            }

            // FIXME: Host-agnostic abstraction
            if options.include_outputs {
                command.arg("--include-outputs");
            }
            if options.use_substitutes {
                command.arg("--use-substitutes");
            }
            if options.gzip {
                command.arg("--gzip");
            }

            command.arg(self.ssh_target()).arg(path.as_path());

            command
        };

        command.env("NIX_SSHOPTS", ssh_options_str);

        command
    }

    fn ssh_options(&self) -> Vec<String> {
        // TODO: Allow configuation of SSH parameters

        let mut options: Vec<String> = [
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "BatchMode=yes",
            "-T",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain(self.extra_ssh_options.clone())
        .collect();

        if let Some(port) = self.port {
            options.push("-p".to_string());
            options.push(port.to_string());
        }

        if let Some(ssh_config) = self.ssh_config.as_ref() {
            options.push("-F".to_string());
            options.push(ssh_config.to_str().unwrap().to_string());
        }

        options
    }

    /// Uploads a single key.
    async fn upload_key(
        &mut self,
        name: &str,
        key: &Key,
        require_ownership: bool,
    ) -> ColmenaResult<()> {
        if let Some(job) = &self.job {
            job.message(format!("Uploading key {}", name))?;
        }

        let path = key.path();
        let key_script = key_uploader::generate_script(key, path, require_ownership);

        let mut command = self.ssh(&["sh", "-c", &key_script]);

        command.stdin(Stdio::piped());
        command.stderr(Stdio::piped());
        command.stdout(Stdio::piped());

        let uploader = command.spawn()?;
        key_uploader::feed_uploader(uploader, key, self.job.clone()).await
    }

    /// Returns the current Boot ID.
    ///
    /// For NixOS, reads from /proc/sys/kernel/random/boot_id.
    /// For Darwin, uses sysctl kern.bootsessionuuid.
    async fn get_boot_id(&mut self, system_type: SystemType) -> ColmenaResult<BootId> {
        let command = match system_type {
            SystemType::NixOS => vec!["cat", "/proc/sys/kernel/random/boot_id"],
            SystemType::Darwin => vec!["sysctl", "-n", "kern.bootsessionuuid"],
        };

        let boot_id = self.ssh(&command).capture_output().await?;

        Ok(BootId(boot_id))
    }

    /// Initiates reboot.
    async fn initate_reboot(&mut self) -> ColmenaResult<()> {
        match self.run_command(self.ssh(&["reboot"])).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if let ColmenaError::ChildFailure { exit_code: 255, .. } = e {
                    // Assume it's "Connection closed by remote host"
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }
}
