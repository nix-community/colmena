use std::fmt;
use std::process::Stdio;

use regex::Regex;
use tokio::process::Command;

use super::{ColmenaError, ColmenaResult};

pub struct NixVersion {
    major: usize,
    minor: usize,
    string: String,
}

impl NixVersion {
    fn parse(string: String) -> Self {
        let re = Regex::new(r" (?P<major>\d+)\.(?P<minor>\d+)").unwrap();
        if let Some(caps) = re.captures(&string) {
            let major = caps.name("major").unwrap().as_str().parse().unwrap();
            let minor = caps.name("minor").unwrap().as_str().parse().unwrap();

            Self {
                major,
                minor,
                string,
            }
        } else {
            Self {
                major: 0,
                minor: 0,
                string: String::from("unknown"),
            }
        }
    }

    fn has_flakes(&self) -> bool {
        self.major > 2 || (self.major == 2 && self.minor >= 4)
    }

    pub fn at_least(&self, major: usize, minor: usize) -> bool {
        self.major >= major && self.minor >= minor
    }
}

impl fmt::Display for NixVersion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.major != 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}???", self.string)
        }
    }
}

pub struct NixCheck {
    version: Option<NixVersion>,
    flakes_supported: bool,
    flakes_enabled: bool,
}

impl NixCheck {
    const NO_NIX: Self = Self {
        version: None,
        flakes_supported: false,
        flakes_enabled: false,
    };

    pub async fn detect() -> Self {
        let version_cmd = Command::new("nix-instantiate")
            .arg("--version")
            .output()
            .await;

        if version_cmd.is_err() {
            return Self::NO_NIX;
        }

        let version =
            NixVersion::parse(String::from_utf8_lossy(&version_cmd.unwrap().stdout).to_string());
        let flakes_supported = version.has_flakes();

        let flake_cmd = Command::new("nix-instantiate")
            .args(["--eval", "-E", "builtins.getFlake"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        if flake_cmd.is_err() {
            return Self::NO_NIX;
        }

        let flakes_enabled = flake_cmd.unwrap().success();

        Self {
            version: Some(version),
            flakes_supported,
            flakes_enabled,
        }
    }

    pub async fn require_flake_support() -> ColmenaResult<()> {
        let check = Self::detect().await;

        if !check.flakes_supported() {
            check.print_flakes_info(true);
            Err(ColmenaError::NoFlakesSupport)
        } else {
            Ok(())
        }
    }

    pub fn print_version_info(&self) {
        if let Some(v) = &self.version {
            tracing::info!("Nix Version: {}", v);
        } else {
            tracing::info!("Nix Version: Not found");
        }
    }

    pub fn print_flakes_info(&self, required: bool) {
        if self.version.is_none() {
            tracing::error!("Nix doesn't appear to be installed.");
            return;
        }

        if self.flakes_enabled {
            tracing::info!("The Nix version you are using supports Flakes and it's enabled.");
        } else if self.flakes_supported {
            tracing::warn!("The Nix version you are using supports Flakes but it's disabled.");
            tracing::warn!(
                "Colmena will automatically enable Flakes for its operations, but you should enable it in your Nix configuration:"
            );
            tracing::warn!("    experimental-features = nix-command flakes");
        } else {
            let emit_log = |s: &str| {
                if required {
                    tracing::error!(s);
                } else {
                    tracing::warn!(s);
                }
            };

            emit_log("The Nix version you are using does not support Flakes.");
            emit_log(
                "If you are using a Nixpkgs version before 21.11, please install nixUnstable for a version that includes Flakes support.",
            );
            if required {
                emit_log("Cannot continue since Flakes support is required for this operation.");
            }
        }
    }

    pub fn flakes_supported(&self) -> bool {
        self.flakes_supported
    }

    pub fn version(&self) -> Option<&NixVersion> {
        self.version.as_ref()
    }
}
