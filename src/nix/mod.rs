use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::ops::Deref;
use std::path::Path;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use validator::{Validate, ValidationError as ValidationErrorType};

use crate::error::{ColmenaError, ColmenaResult};

pub mod command;
pub use command::NixCommand;

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

    #[serde(rename = "sshProto")]
    ssh_proto: String, // TODO: Use Enum

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

    /// Options to pass as --option name value.
    ///
    /// Sorted so the rendered flags are deterministic.
    options: BTreeMap<String, String>,
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

    pub fn to_ssh_host(&self, nix_flags: NixFlags) -> Option<Ssh> {
        self.target_host.as_ref().map(|target_host| {
            let mut host = Ssh::new(self.target_user.clone(), target_host.clone(), nix_flags);
            host.set_privilege_escalation_command(self.privilege_escalation_command.clone());
            host.set_extra_ssh_options(self.extra_ssh_options.clone());
            host.set_ssh_proto(self.ssh_proto.clone());

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

    pub fn add_option(&mut self, name: String, value: String) {
        self.options.insert(name, value);
    }

    pub fn has_option(&self, name: &str) -> bool {
        self.options.contains_key(name)
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
