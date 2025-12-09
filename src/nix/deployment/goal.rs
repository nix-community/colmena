//! Deployment goals.

use std::str::FromStr;

/// The goal of a deployment.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Goal {
    /// Build the configurations only.
    Build,

    /// Push the closures only.
    Push,

    /// Make the configuration the boot default and activate now.
    #[default]
    Switch,

    /// Make the configuration the boot default.
    Boot,

    /// Activate the configuration, but don't make it the boot default.
    Test,

    /// Show what would be done if this configuration were activated.
    DryActivate,

    /// Only upload keys.
    UploadKeys,
}

impl FromStr for Goal {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "build" => Ok(Self::Build),
            "push" => Ok(Self::Push),
            "switch" => Ok(Self::Switch),
            "boot" => Ok(Self::Boot),
            "test" => Ok(Self::Test),
            "dry-activate" => Ok(Self::DryActivate),
            "keys" => Ok(Self::UploadKeys),
            _ => Err("Not one of [build, push, switch, boot, test, dry-activate, keys]."),
        }
    }
}

impl std::fmt::Display for Goal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Build => "build",
            Self::Push => "push",
            Self::Switch => "switch",
            Self::Boot => "boot",
            Self::Test => "test",
            Self::DryActivate => "dry-activate",
            Self::UploadKeys => "keys",
        })
    }
}

impl Goal {
    pub fn as_str(&self) -> Option<&'static str> {
        use Goal::*;
        match self {
            Build => None,
            Push => None,
            Switch => Some("switch"),
            Boot => Some("boot"),
            Test => Some("test"),
            DryActivate => Some("dry-activate"),
            UploadKeys => Some("keys"),
        }
    }

    pub fn success_str(&self) -> &'static str {
        use Goal::*;
        match self {
            Build => "Configuration built",
            Push => "Pushed",
            Switch => "Activation successful",
            Boot => "Will be activated next boot",
            Test => "Activation successful (test)",
            DryActivate => "Dry activation successful",
            UploadKeys => "Uploaded keys",
        }
    }

    pub fn should_switch_profile(&self) -> bool {
        use Goal::*;
        matches!(self, Boot | Switch)
    }

    pub fn requires_activation(&self) -> bool {
        use Goal::*;
        !matches!(self, Build | UploadKeys | Push)
    }

    pub fn persists_after_reboot(&self) -> bool {
        use Goal::*;
        matches!(self, Switch | Boot)
    }

    pub fn requires_target_host(&self) -> bool {
        use Goal::*;
        !matches!(self, Build)
    }

    /// Returns whether this goal is supported for Darwin systems.
    ///
    /// Darwin doesn't support the "boot" goal since it doesn't have
    /// separate boot profiles like NixOS.
    pub fn supported_on_darwin(&self) -> bool {
        use Goal::*;
        !matches!(self, Boot)
    }
}
