//! Static files required to evaluate a Hive configuration.
//!
//! We embed Nix expressions (eval.nix, options.nix, modules.nix) into the
//! resulting binary to ease distribution. The files are written to a
//! temporary path when we need to use them.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use tempfile::{Builder as TempFileBuilder, TempDir};

use crate::error::ColmenaResult;

const EVAL_NIX: &[u8] = include_bytes!("eval.nix");
const OPTIONS_NIX: &[u8] = include_bytes!("options.nix");
const MODULES_NIX: &[u8] = include_bytes!("modules.nix");

/// Static files required to evaluate a Hive configuration.
#[derive(Debug)]
pub(super) struct Assets {
    /// Temporary directory holding the files.
    temp_dir: TempDir,
}

impl Assets {
    pub fn new() -> ColmenaResult<Self> {
        let temp_dir = TempFileBuilder::new().prefix("colmena-assets-").tempdir()?;

        create_file(&temp_dir, "eval.nix", EVAL_NIX)?;
        create_file(&temp_dir, "options.nix", OPTIONS_NIX)?;
        create_file(&temp_dir, "modules.nix", MODULES_NIX)?;

        Ok(Self { temp_dir })
    }

    /// Returns the base expression from which the evaluated `hive.nix` can be used.
    pub fn get_base_expression(&self, hive_nix: &Path) -> String {
        format!(
            "with builtins; let eval = import {eval_nix}; hive = eval {{ rawHive = import {path}; colmenaOptions = import {options_nix}; colmenaModules = import {modules_nix}; }}; in ",
            path = hive_nix.to_str().unwrap(),
            eval_nix = self.get_path("eval.nix"),
            options_nix = self.get_path("options.nix"),
            modules_nix = self.get_path("modules.nix"),
        )
    }

    fn get_path(&self, name: &str) -> String {
        self.temp_dir
            .path()
            .join(name)
            .to_str()
            .unwrap()
            .to_string()
    }
}

fn create_file(base: &TempDir, name: &str, contents: &[u8]) -> ColmenaResult<()> {
    let path = base.path().join(name);
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;

    f.write_all(contents)?;

    Ok(())
}
