use std::convert::TryFrom;
use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use super::{
    BuildResult, ColmenaError, ColmenaResult, Goal, StoreDerivation, StorePath, SystemType,
};

pub type ProfileDerivation = StoreDerivation<Profile>;

/// A system profile (NixOS or nix-darwin).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile(StorePath);

impl Profile {
    /// Returns the command to activate this profile for NixOS.
    fn activation_command_nixos(&self, goal: Goal) -> Option<Vec<String>> {
        if let Some(goal_str) = goal.as_str() {
            let path = self.as_path().join("bin/switch-to-configuration");
            let switch_to_configuration = path
                .to_str()
                .expect("The string should be UTF-8 valid")
                .to_string();

            Some(vec![switch_to_configuration, goal_str.to_string()])
        } else {
            None
        }
    }

    /// Returns the command to activate this profile for Darwin.
    ///
    /// Uses `sw/bin/darwin-rebuild` to activate the configuration. Darwin has no
    /// separate boot profile, so the `Boot` goal is unsupported (returns `None`,
    /// via the catch-all — callers gate on [`Goal::supported_on_darwin`]).
    fn activation_command_darwin(&self, goal: Goal) -> Option<Vec<String>> {
        let darwin_rebuild_path = self.as_path().join("sw/bin/darwin-rebuild");
        let darwin_rebuild = darwin_rebuild_path
            .to_str()
            .expect("The string should be UTF-8 valid")
            .to_string();

        match goal {
            // `darwin-rebuild activate` applies the configuration.
            Goal::Switch | Goal::Test => Some(vec![darwin_rebuild, "activate".to_string()]),
            // `darwin-rebuild check` validates without applying.
            Goal::DryActivate => Some(vec![darwin_rebuild, "check".to_string()]),
            _ => None,
        }
    }

    /// Returns the command to activate this profile based on system type.
    pub fn activation_command(&self, goal: Goal, system_type: SystemType) -> Option<Vec<String>> {
        match system_type {
            SystemType::NixOS => self.activation_command_nixos(goal),
            SystemType::Darwin => self.activation_command_darwin(goal),
        }
    }

    /// Returns the store path.
    pub fn as_store_path(&self) -> &StorePath {
        &self.0
    }

    /// Returns the raw store path.
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    /// Create a GC root for this profile.
    pub async fn create_gc_root(&self, path: &Path) -> ColmenaResult<()> {
        let mut command = Command::new("nix-store");
        command.args([
            "--no-build-output",
            "--indirect",
            "--add-root",
            path.to_str().unwrap(),
        ]);
        command.args(["--realise", self.as_path().to_str().unwrap()]);
        command.stdout(Stdio::null());

        let status = command.status().await?;
        if !status.success() {
            return Err(status.into());
        }

        Ok(())
    }

    pub(super) fn from_store_path_unchecked(path: StorePath) -> Self {
        Self(path)
    }
}

impl TryFrom<BuildResult<Profile>> for Profile {
    type Error = ColmenaError;

    fn try_from(result: BuildResult<Self>) -> ColmenaResult<Self> {
        let paths = result.paths();

        if paths.is_empty() {
            return Err(ColmenaError::BadOutput {
                output: String::from("There is no store path"),
            });
        }

        if paths.len() > 1 {
            return Err(ColmenaError::BadOutput {
                output: String::from("Build resulted in more than 1 store path"),
            });
        }

        let path = paths.iter().next().unwrap().to_owned();

        Ok(Self::from_store_path_unchecked(path))
    }
}
