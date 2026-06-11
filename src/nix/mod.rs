use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Deref;
use std::path::Path;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use validator::{Validate, ValidationError as ValidationErrorType};

use crate::error::{ColmenaError, ColmenaResult};

pub mod host;
use host::Ssh;
pub use host::{CopyDirection, CopyOptions, Host, RebootOptions};

pub mod hive;
pub use hive::{Hive, HivePath};

pub mod store;
pub use store::{BuildResult, StoreDerivation, StorePath};

pub mod key;
pub use key::Key;

pub mod profile;
pub use profile::{Profile, ProfileDerivation};

pub mod deployment;
pub use deployment::Goal;

pub mod info;
pub use info::NixCheck;

pub mod flake;
pub use flake::Flake;

pub mod node_filter;
pub use node_filter::NodeFilter;

pub mod evaluator;

pub mod expression;
pub use expression::{NixExpression, SerializedNixExpression};

/// Path to the main system profile.
pub const SYSTEM_PROFILE: &str = "/nix/var/nix/profiles/system";

/// Path to the system profile that's currently active.
pub const CURRENT_PROFILE: &str = "/run/current-system";

/// A node's attribute name.
#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(transparent)]
pub struct NodeName(#[serde(deserialize_with = "NodeName::deserialize")] String);

#[derive(Debug, Clone, Validate, Deserialize)]
pub struct NodeConfig {
    #[serde(rename = "targetHost")]
    target_host: Option<String>,

    #[serde(rename = "targetUser")]
    target_user: Option<String>,

    #[serde(rename = "targetPort")]
    target_port: Option<u16>,

    #[serde(rename = "allowLocalDeployment")]
    allow_local_deployment: bool,

    #[serde(rename = "buildOnTarget")]
    build_on_target: bool,

    tags: Vec<String>,

    #[serde(rename = "replaceUnknownProfiles")]
    replace_unknown_profiles: bool,

    #[serde(rename = "privilegeEscalationCommand")]
    privilege_escalation_command: Vec<String>,

    #[serde(rename = "sshOptions")]
    extra_ssh_options: Vec<String>,

    #[validate(custom(function = "validate_keys"))]
    keys: HashMap<String, Key>,
}

#[derive(Debug, Clone, Validate, Deserialize)]
pub struct MetaConfig {
    #[serde(rename = "allowApplyAll")]
    pub allow_apply_all: bool,

    #[serde(rename = "machinesFile")]
    pub machines_file: Option<String>,
}

/// Nix CLI flags.
#[derive(Debug, Clone, Default)]
pub struct NixFlags {
    /// Whether to pass --show-trace.
    show_trace: bool,

    /// Whether to pass --pure-eval.
    pure_eval: bool,

    /// Whether to pass --impure.
    impure: bool,

    /// Designated builders.
    ///
    /// See <https://nixos.org/manual/nix/stable/advanced-topics/distributed-builds.html>.
    ///
    /// Valid examples:
    /// - `@/path/to/machines`
    /// - `builder@host.tld riscv64-linux /home/nix/.ssh/keys/builder.key 8 1 kvm`
    builders: Option<String>,

    /// Options to pass as --option name value.
    options: HashMap<String, String>,
}

impl NodeName {
    /// Returns the string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Creates a NodeName from a String.
    pub fn new(name: String) -> ColmenaResult<Self> {
        let validated = Self::validate(name)?;
        Ok(Self(validated))
    }

    /// Deserializes a potentially-invalid node name.
    fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        use de::Error;
        String::deserialize(deserializer)
            .and_then(|s| Self::validate(s).map_err(|e| Error::custom(e.to_string())))
    }

    fn validate(s: String) -> ColmenaResult<String> {
        // FIXME: Elaborate
        if s.is_empty() {
            return Err(ColmenaError::EmptyNodeName);
        }

        Ok(s)
    }
}

impl Deref for NodeName {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl NodeConfig {
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn allows_local_deployment(&self) -> bool {
        self.allow_local_deployment
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn privilege_escalation_command(&self) -> &Vec<String> {
        &self.privilege_escalation_command
    }

    pub fn build_on_target(&self) -> bool {
        self.build_on_target
    }
    pub fn set_build_on_target(&mut self, enable: bool) {
        self.build_on_target = enable;
    }

    pub fn to_ssh_host(&self) -> Option<Ssh> {
        self.target_host.as_ref().map(|target_host| {
            let mut host = Ssh::new(self.target_user.clone(), target_host.clone());
            host.set_privilege_escalation_command(self.privilege_escalation_command.clone());
            host.set_extra_ssh_options(self.extra_ssh_options.clone());

            if let Some(target_port) = self.target_port {
                host.set_port(target_port);
            }

            host
        })
    }
}

impl NixFlags {
    pub fn set_show_trace(&mut self, show_trace: bool) {
        self.show_trace = show_trace;
    }

    pub fn set_pure_eval(&mut self, pure_eval: bool) {
        self.pure_eval = pure_eval;
    }

    pub fn set_impure(&mut self, impure: bool) {
        self.impure = impure;
    }

    pub fn set_builders(&mut self, builders: Option<String>) {
        self.builders = builders;
    }

    pub fn set_options(&mut self, options: HashMap<String, String>) {
        self.options = options;
    }

    pub fn to_args(&self) -> Vec<String> {
        self.to_args_inner(false)
    }

    /// Returns arguments for `nix-store`.
    pub fn to_nix_store_args(&self) -> Vec<String> {
        self.to_args_inner(true)
    }

    fn to_args_inner(&self, nix_store: bool) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(builders) = &self.builders {
            args.append(&mut vec![
                "--option".to_string(),
                "builders".to_string(),
                builders.clone(),
            ]);
        }

        if self.show_trace {
            args.push("--show-trace".to_string());
        }

        if self.pure_eval {
            args.push("--pure-eval".to_string());
        }

        // The `nix-store` command does not accept `--impure`
        // TODO: Not happy about this solution - Have a better Nix abstraction that hides
        // CLI details (e.g., nix3 CLI differences)
        if self.impure && !nix_store {
            args.push("--impure".to_string());
        }

        for (name, value) in self.options.iter() {
            args.push("--option".to_string());
            args.push(name.to_string());
            args.push(value.to_string());
        }

        args
    }
}

fn validate_keys(keys: &HashMap<String, Key>) -> Result<(), ValidationErrorType> {
    // Bad secret names:
    // - /etc/passwd
    // - ../../../../../etc/passwd

    for name in keys.keys() {
        let path = Path::new(name);
        if path.has_root() {
            return Err(ValidationErrorType::new(
                "Secret key name cannot be absolute",
            ));
        }

        if path.components().count() != 1 {
            return Err(ValidationErrorType::new(
                "Secret key name cannot contain path separators",
            ));
        }
    }
    Ok(())
}
