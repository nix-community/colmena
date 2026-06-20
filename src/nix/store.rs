use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::host::AnyHost;
use super::Host;
use crate::error::{ColmenaError, ColmenaResult};
use crate::util::CommandExt;

/// A Nix store path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorePath(PathBuf);

/// A store derivation (.drv) that will result in a T when built.
#[derive(Debug)]
pub struct StoreDerivation<T: TryFrom<BuildResult<T>>> {
    path: StorePath,
    _target: PhantomData<T>,
}

/// Results of a build/realization.
pub struct BuildResult<T: TryFrom<BuildResult<T>>> {
    results: Vec<StorePath>,
    _derivation: PhantomData<T>,
}

impl StorePath {
    /// Returns the raw store path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Determines whether the path points to a derivation.
    pub fn is_derivation(&self) -> bool {
        if let Some(ext) = self.0.extension() {
            ext == "drv"
        } else {
            false
        }
    }

    /// Returns the immediate dependencies of the store path.
    pub async fn references(&self) -> ColmenaResult<Vec<StorePath>> {
        let references = Command::new("nix-store")
            .args(["--query", "--references"])
            .arg(&self.0)
            .capture_output()
            .await?
            .trim_end()
            .split('\n')
            .map(|p| StorePath(PathBuf::from(p)))
            .collect();

        Ok(references)
    }

    /// Converts the store path into a store derivation.
    pub fn into_derivation<T: TryFrom<BuildResult<T>>>(self) -> ColmenaResult<StoreDerivation<T>> {
        if self.is_derivation() {
            Ok(StoreDerivation::<T>::from_store_path_unchecked(self))
        } else {
            Err(ColmenaError::NotADerivation { store_path: self })
        }
    }
}

impl Deref for StorePath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<String> for StorePath {
    type Error = ColmenaError;

    fn try_from(s: String) -> ColmenaResult<Self> {
        if s.starts_with("/nix/store/") {
            Ok(Self(s.into()))
        } else {
            Err(ColmenaError::InvalidStorePath)
        }
    }
}

impl From<StorePath> for PathBuf {
    fn from(sp: StorePath) -> Self {
        sp.0
    }
}

impl<T: TryFrom<BuildResult<T>>> Clone for StoreDerivation<T> {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            _target: PhantomData,
        }
    }
}

impl<T: TryFrom<BuildResult<T>>> StoreDerivation<T> {
    /// Returns the store path.
    pub fn as_store_path(&self) -> &StorePath {
        &self.path
    }

    fn from_store_path_unchecked(path: StorePath) -> Self {
        Self {
            path,
            _target: PhantomData,
        }
    }
}

impl<T: TryFrom<BuildResult<T>, Error = ColmenaError>> StoreDerivation<T> {
    /// Builds the store derivation on a host, resulting in a T.
    pub async fn realize(&self, host: &mut AnyHost) -> ColmenaResult<T> {
        let paths: Vec<StorePath> = host.realize(&self.path).await?;

        let result = BuildResult {
            results: paths,
            _derivation: PhantomData,
        };
        result.try_into()
    }

    /// Builds the store derivation on a host without copying the results back.
    pub async fn realize_remote(&self, host: &mut AnyHost) -> ColmenaResult<T> {
        let paths: Vec<StorePath> = host.realize_remote(&self.path).await?;

        let result = BuildResult {
            results: paths,
            _derivation: PhantomData,
        };
        result.try_into()
    }
}

impl<T: TryFrom<BuildResult<T>>> fmt::Display for StoreDerivation<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.path)
    }
}

impl<T: TryFrom<BuildResult<T>, Error = ColmenaError>> BuildResult<T> {
    pub fn paths(&self) -> &[StorePath] {
        self.results.as_slice()
    }
}
