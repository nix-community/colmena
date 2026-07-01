use std::io::Write;

use tempfile::Builder as TempFileBuilder;
use tokio::process::Command;

use crate::error::ColmenaResult;
use crate::nix::Hive;
use crate::nix::info::NixCheck;

pub async fn run(hive: Hive) -> ColmenaResult<()> {
    let nix_check = NixCheck::detect().await;
    let nix_version = nix_check.version().expect("Could not detect Nix version");

    let expr = hive.get_repl_expression();

    let mut expr_file = TempFileBuilder::new()
        .prefix("colmena-repl-")
        .suffix(".nix")
        .tempfile()?;

    expr_file.write_all(expr.as_bytes())?;

    let mut repl_cmd = Command::new("nix");

    repl_cmd.arg("repl");

    if nix_version.at_least(2, 4) {
        // `nix repl` is expected to be marked as experimental:
        // <https://github.com/NixOS/nix/issues/5604>
        repl_cmd.args(["--extra-experimental-features", "nix-command flakes"]);
    }

    if nix_version.at_least(2, 10) {
        repl_cmd.arg("--file");
    }

    let status = repl_cmd.arg(expr_file.path()).status().await?;

    if !status.success() {
        return Err(status.into());
    }

    Ok(())
}
