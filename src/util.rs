use std::convert::TryFrom;

use std::process::Stdio;

use futures::future::join3;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

use super::error::{ColmenaError, ColmenaResult};
use super::job::JobHandle;
use super::nix::deployment::TargetNodeMap;
use super::nix::StorePath;

const NEWLINE: u8 = 0xa;

/// Non-interactive execution of an arbitrary command.
pub struct CommandExecution {
    command: Command,
    job: Option<JobHandle>,
    hide_stdout: bool,
    stdout: Option<String>,
    stderr: Option<String>,
}

/// Helper extensions for Commands.
pub trait CommandExt {
    /// Runs the command with stdout and stderr passed through to the user.
    async fn passthrough(&mut self) -> ColmenaResult<()>;

    /// Runs the command, capturing the output as a String.
    async fn capture_output(&mut self) -> ColmenaResult<String>;

    /// Runs the command, capturing deserialized output from JSON.
    async fn capture_json<T>(&mut self) -> ColmenaResult<T>
    where
        T: DeserializeOwned;

    /// Runs the command, capturing a single store path.
    async fn capture_store_path(&mut self) -> ColmenaResult<StorePath>;
}

impl CommandExecution {
    pub fn new(command: Command) -> Self {
        Self {
            command,
            job: None,
            hide_stdout: false,
            stdout: None,
            stderr: None,
        }
    }

    /// Sets the job associated with this execution.
    pub fn set_job(&mut self, job: Option<JobHandle>) {
        self.job = job;
    }

    /// Sets whether to hide stdout.
    pub fn set_hide_stdout(&mut self, hide_stdout: bool) {
        self.hide_stdout = hide_stdout;
    }

    /// Returns logs from the last invocation.
    pub fn get_logs(&self) -> (Option<&String>, Option<&String>) {
        (self.stdout.as_ref(), self.stderr.as_ref())
    }

    /// Runs the command.
    pub async fn run(&mut self) -> ColmenaResult<()> {
        self.command.stdin(Stdio::null());
        self.command.stdout(Stdio::piped());
        self.command.stderr(Stdio::piped());

        self.stdout = Some(String::new());
        self.stderr = Some(String::new());

        let mut child = self.command.spawn()?;

        let stdout = BufReader::new(child.stdout.take().unwrap());
        let stderr = BufReader::new(child.stderr.take().unwrap());

        let stdout_job = if self.hide_stdout {
            None
        } else {
            self.job.clone()
        };

        let futures = join3(
            capture_stream(stdout, stdout_job, false),
            capture_stream(stderr, self.job.clone(), true),
            child.wait(),
        );

        let (stdout, stderr, wait) = futures.await;
        self.stdout = Some(stdout?);
        self.stderr = Some(stderr?);

        let exit = wait?;

        if exit.success() {
            Ok(())
        } else {
            Err(exit.into())
        }
    }
}

impl CommandExt for Command {
    /// Runs the command with stdout and stderr passed through to the user.
    async fn passthrough(&mut self) -> ColmenaResult<()> {
        let exit = self.spawn()?.wait().await?;

        if exit.success() {
            Ok(())
        } else {
            Err(exit.into())
        }
    }

    /// Captures output as a String.
    async fn capture_output(&mut self) -> ColmenaResult<String> {
        // We want the user to see the raw errors
        let output = self
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?
            .wait_with_output()
            .await?;

        if output.status.success() {
            // FIXME: unwrap
            Ok(String::from_utf8(output.stdout).unwrap())
        } else {
            Err(output.status.into())
        }
    }

    /// Captures deserialized output from JSON.
    async fn capture_json<T>(&mut self) -> ColmenaResult<T>
    where
        T: DeserializeOwned,
    {
        let output = self.capture_output().await?;
        serde_json::from_str(&output).map_err(|_| ColmenaError::BadOutput {
            output: output.clone(),
        })
    }

    /// Captures a single store path.
    async fn capture_store_path(&mut self) -> ColmenaResult<StorePath> {
        let output = self.capture_output().await?;
        let path = output.trim_end().to_owned();
        StorePath::try_from(path)
    }
}

impl CommandExt for CommandExecution {
    async fn passthrough(&mut self) -> ColmenaResult<()> {
        self.run().await
    }

    /// Captures output as a String.
    async fn capture_output(&mut self) -> ColmenaResult<String> {
        self.run().await?;
        let (stdout, _) = self.get_logs();

        Ok(stdout.unwrap().to_owned())
    }

    /// Captures deserialized output from JSON.
    async fn capture_json<T>(&mut self) -> ColmenaResult<T>
    where
        T: DeserializeOwned,
    {
        let output = self.capture_output().await?;
        serde_json::from_str(&output).map_err(|_| ColmenaError::BadOutput {
            output: output.clone(),
        })
    }

    /// Captures a single store path.
    async fn capture_store_path(&mut self) -> ColmenaResult<StorePath> {
        let output = self.capture_output().await?;
        let path = output.trim_end().to_owned();
        StorePath::try_from(path)
    }
}

pub async fn capture_stream<R>(
    mut stream: BufReader<R>,
    job: Option<JobHandle>,
    stderr: bool,
) -> ColmenaResult<String>
where
    R: AsyncRead + Unpin,
{
    let mut log = String::new();

    loop {
        let mut line = Vec::new();
        let len = stream.read_until(NEWLINE, &mut line).await?;
        let line = String::from_utf8_lossy(&line);

        if len == 0 {
            break;
        }

        let trimmed = line.trim_end();

        if let Some(job) = &job {
            if stderr {
                job.stderr(trimmed.to_string())?;
            } else {
                job.stdout(trimmed.to_string())?;
            }
        }

        log += trimmed;
        log += "\n";
    }

    Ok(log)
}

pub fn get_label_width(targets: &TargetNodeMap) -> Option<usize> {
    targets.keys().map(|n| n.len()).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::BufReader;
    use tokio_test::block_on;

    #[test]
    fn test_capture_stream() {
        let expected = "Hello\nWorld\n";

        let stream = BufReader::new(expected.as_bytes());
        let captured = block_on(async { capture_stream(stream, None, false).await.unwrap() });

        assert_eq!(expected, captured);
    }

    #[test]
    fn test_capture_stream_with_invalid_utf8() {
        let stream = BufReader::new([0x80, 0xa].as_slice());
        let captured = block_on(async { capture_stream(stream, None, false).await.unwrap() });

        assert_eq!("\u{fffd}\n", captured);
    }
}
