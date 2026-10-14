//! Utilities for using the key uploader script.
//!
//! The key uploader is a simple shell script that reads the contents
//! of the secret file from stdin into a temporary file then atomically
//! replaces the destination file with the temporary file.

use std::borrow::Cow;
use std::path::Path;

use futures::future::join3;
use shell_escape::unix::escape;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::error::{ColmenaError, ColmenaResult};
use crate::job::JobHandle;
use crate::nix::Key;
use crate::util::capture_stream;

const SCRIPT_TEMPLATE: &str = include_str!("./key_uploader.template.sh");

pub fn generate_script<'a>(
    key: &'a Key,
    destination: &'a Path,
    require_ownership: bool,
) -> Cow<'a, str> {
    let key_script = SCRIPT_TEMPLATE
        .to_string()
        .replace("%DESTINATION%", destination.to_str().unwrap())
        .replace("%USER%", &escape(key.user().into()))
        .replace("%GROUP%", &escape(key.group().into()))
        .replace("%PERMISSIONS%", &escape(key.permissions().into()))
        .replace(
            "%REQUIRE_OWNERSHIP%",
            if require_ownership { "1" } else { "" },
        )
        .trim_end_matches('\n')
        .to_string();

    escape(key_script.into())
}

pub async fn feed_uploader(
    mut uploader: Command,
    key: &Key,
    job: Option<JobHandle>,
) -> ColmenaResult<()> {
    let mut reader = key.reader().await.map_err(|error| ColmenaError::KeyError {
        name: key.name().to_owned(),
        error,
    })?;

    // Start the uploader only once the key is ready. If the uploader got EOF
    // instead, it would replace the key on the target with an empty file.
    let mut uploader = uploader.spawn()?;
    let mut stdin = uploader.stdin.take().unwrap();

    tokio::io::copy(reader.as_mut(), &mut stdin).await?;
    stdin.flush().await?;
    drop(stdin);

    let stdout = BufReader::new(uploader.stdout.take().unwrap());
    let stderr = BufReader::new(uploader.stderr.take().unwrap());

    let futures = join3(
        capture_stream(stdout, job.clone(), false),
        capture_stream(stderr, job.clone(), true),
        uploader.wait(),
    );
    let (stdout, stderr, exit) = futures.await;
    stdout?;
    stderr?;

    let exit = exit?;

    if exit.success() {
        Ok(())
    } else {
        Err(exit.into())
    }
}
