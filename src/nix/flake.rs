//! Nix Flake utilities.

use std::convert::AsRef;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;

use super::{ColmenaError, ColmenaResult, NixCommand, NixFlags};

/// A Nix Flake.
#[derive(Debug)]
pub struct Flake {
    /// The flake metadata.
    metadata: FlakeMetadata,

    /// The directory the flake lives in, if it's a local flake.
    local_dir: Option<PathBuf>,
}

/// A `nix flake metadata --json` invocation.
#[derive(Deserialize, Debug)]
struct FlakeMetadata {
    /// The resolved URL of the flake.
    #[serde(rename = "resolvedUrl")]
    resolved_url: String,
}

impl Flake {
    /// Creates a flake from the given directory.
    ///
    /// This will try to retrieve the resolved URL of the local flake
    /// in the specified directory.
    pub async fn from_dir<P: AsRef<Path>>(dir: P, flags: &NixFlags) -> ColmenaResult<Self> {
        let flake = dir
            .as_ref()
            .as_os_str()
            .to_str()
            .expect("Flake directory path contains non-UTF-8 characters");

        let metadata = FlakeMetadata::resolve(flake, flags).await?;

        Ok(Self {
            metadata,
            local_dir: Some(dir.as_ref().to_owned()),
        })
    }

    /// Creates a flake from a Flake URI.
    pub async fn from_uri(uri: impl AsRef<str>, flags: &NixFlags) -> ColmenaResult<Self> {
        let metadata = FlakeMetadata::resolve(uri.as_ref(), flags).await?;

        Ok(Self {
            metadata,
            local_dir: None,
        })
    }

    /// Returns the URI.
    pub fn uri(&self) -> &str {
        &self.metadata.resolved_url
    }

    /// Returns the local directory, if it exists.
    pub fn local_dir(&self) -> Option<&Path> {
        self.local_dir.as_deref()
    }
}

impl FlakeMetadata {
    /// Resolves a flake.
    async fn resolve(flake: &str, flags: &NixFlags) -> ColmenaResult<Self> {
        let child = NixCommand::nix(flags.clone())
            .args(["flake", "metadata", "--json", "--no-write-lock-file"])
            .arg(flake)
            .build()
            .stdout(Stdio::piped())
            .spawn()?;

        let output = child.wait_with_output().await?;

        if !output.status.success() {
            return Err(output.status.into());
        }

        serde_json::from_slice::<FlakeMetadata>(&output.stdout).map_err(|_| {
            let output = String::from_utf8_lossy(&output.stdout).to_string();
            ColmenaError::BadOutput { output }
        })
    }
}
