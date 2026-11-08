use std::{
    convert::TryFrom,
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use tokio::process::Command;
use validator::{Validate, ValidationError};

#[non_exhaustive]
#[derive(Debug, Snafu)]
pub enum KeyError {
    #[snafu(display("I/O Error: {}", error))]
    IoError { error: io::Error },
    #[snafu(display("Key command failed: {}, stderr: {}", status, stderr))]
    KeyCommandStatus { status: ExitStatus, stderr: String },
}

impl From<std::io::Error> for KeyError {
    fn from(error: std::io::Error) -> Self {
        Self::IoError { error }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "KeySources")]
enum KeySource {
    #[serde(rename = "text")]
    Text(String),

    #[serde(rename = "keyCommand")]
    Command(Vec<String>),

    #[serde(rename = "keyFile")]
    File(PathBuf),
}

impl TryFrom<KeySources> for KeySource {
    type Error = String;

    fn try_from(ks: KeySources) -> Result<Self, Self::Error> {
        match (ks.text, ks.command, ks.file) {
            (Some(text), None, None) => Ok(KeySource::Text(text)),
            (None, Some(command), None) => Ok(KeySource::Command(command)),
            (None, None, Some(file)) => Ok(KeySource::File(file)),
            x => Err(format!(
                "Somehow 0 or more than 1 key source was specified: {:?}",
                x
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeySources {
    text: Option<String>,

    #[serde(rename = "keyCommand")]
    command: Option<Vec<String>>,

    #[serde(rename = "keyFile")]
    file: Option<PathBuf>,
}

/// When to upload a given key.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadAt {
    /// Before activating the system profile.
    #[serde(rename = "pre-activation")]
    PreActivation,

    /// After successfully activating the system profile.
    #[serde(rename = "post-activation")]
    PostActivation,
}

#[derive(Debug, Clone, Validate, Serialize, Deserialize)]
pub struct Key {
    name: String,

    path: PathBuf,

    #[serde(flatten)]
    source: KeySource,

    #[validate(custom(function = "validate_dest_dir"))]
    #[serde(rename = "destDir")]
    dest_dir: PathBuf,

    #[validate(custom(function = "validate_unix_name"))]
    user: String,

    #[validate(custom(function = "validate_unix_name"))]
    group: String,

    permissions: String,

    #[serde(rename = "uploadAt")]
    upload_at: UploadAt,
}

impl Key {
    pub async fn read(&self) -> Result<Vec<u8>, KeyError> {
        match &self.source {
            KeySource::Text(content) => Ok(content.clone().into_bytes()),
            KeySource::Command(command) => {
                let pathname = &command[0];
                let argv = &command[1..];

                let output = Command::new(pathname)
                    .args(argv)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?
                    .wait_with_output()
                    .await?;

                if output.status.success() {
                    Ok(output.stdout)
                } else {
                    Err(KeyError::KeyCommandStatus {
                        status: output.status,
                        stderr: std::str::from_utf8(&output.stderr)
                            .unwrap_or_default()
                            .trim_end()
                            .into(),
                    })
                }
            }
            KeySource::File(path) => Ok(tokio::fs::read(path).await?),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn user(&self) -> &str {
        &self.user
    }
    pub fn group(&self) -> &str {
        &self.group
    }
    pub fn permissions(&self) -> &str {
        &self.permissions
    }
    pub fn upload_at(&self) -> UploadAt {
        self.upload_at
    }
}

fn validate_unix_name(name: &str) -> Result<(), ValidationError> {
    // systemd's strict user/group name syntax: uppercase/lowercase letters,
    // digits, underscores and hyphens, not starting with a digit or hyphen,
    // capped at 31 characters. NixOS itself only enforces the length limit
    // (nixos/modules/config/users-groups.nix), so this is the more specific
    // rule. https://github.com/systemd/systemd/blob/main/docs/USER_NAMES.md#strict-mode
    let re = Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_-]{0,30}$").unwrap();
    if re.is_match(name) {
        Ok(())
    } else {
        Err(ValidationError::new("Invalid user/group name"))
    }
}

fn validate_dest_dir(dir: &Path) -> Result<(), ValidationError> {
    if dir.has_root() {
        Ok(())
    } else {
        Err(ValidationError::new(
            "Secret key destination directory must be absolute",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_unix_name_accepts_underscore() {
        assert!(validate_unix_name("foo_bar").is_ok());
        assert!(validate_unix_name("_foo").is_ok());
        assert!(validate_unix_name("foo-bar").is_ok());
        assert!(validate_unix_name("foo").is_ok());
        assert!(validate_unix_name("Foo").is_ok());
        assert!(validate_unix_name(&"a".repeat(31)).is_ok());
    }

    #[test]
    fn test_validate_unix_name_rejects_invalid() {
        assert!(validate_unix_name("-foo").is_err());
        assert!(validate_unix_name("1foo").is_err());
        assert!(validate_unix_name("").is_err());
        assert!(validate_unix_name("foo123$").is_err());
        assert!(validate_unix_name(&"a".repeat(32)).is_err());
    }
}
