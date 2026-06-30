//! Job control.
//!
//! We use a channel to send Events from different futures to a job monitor,
//! which coordinates the display of progress onto the terminal.

use std::collections::HashMap;
use std::fmt::{self, Display, Write};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::future::Either;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time;
use tracing::{Instrument, Span};
use uuid::Uuid;

use crate::error::{ColmenaError, ColmenaResult};
use crate::nix::NodeName;
use crate::progress::{Line, LineStyle, Message as ProgressMessage, Sender as ProgressSender};

pub type Sender = UnboundedSender<Event>;
pub type Receiver = UnboundedReceiver<Event>;

/// A handle to a job.
pub type JobHandle = Arc<JobHandleInner>;

/// Maximum log lines to print for failures.
const LOG_CONTEXT_LINES: usize = 20;

/// An opaque job identifier.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct JobId(Uuid);

/// Coordinator of all job states.
///
/// It receives event messages from jobs and updates the progress
/// spinners.
pub struct JobMonitor {
    /// The receiving end of the mpsc channel.
    receiver: Receiver,

    /// Events received so far.
    events: Vec<Event>,

    /// Known jobs and their metadata.
    jobs: HashMap<JobId, JobMetadata>,

    /// ID of the meta job.
    meta_job_id: JobId,

    /// Sender to the spinner thread.
    progress: Option<ProgressSender>,

    /// Estimated max label size.
    label_width: Option<usize>,
}

/// The state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Waiting to begin.
    ///
    /// Progress bar is not shown in this state.
    Waiting,

    /// Running.
    Running,

    /// Succeeded.
    Succeeded,

    /// Failed.
    Failed,
}

/// The type of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobType {
    /// Meta.
    Meta,

    /// Nix evaluation.
    Evaluate,

    /// Nix build.
    Build,

    /// Key uploading.
    UploadKeys,

    /// Pushing closure to a host.
    Push,

    /// Activating a system profile on a host.
    Activate,

    /// Executing an arbitrary command.
    Execute,

    /// Creating GC roots.
    CreateGcRoots,

    /// Rebooting a host.
    Reboot,
}

/// A handle to a job.
///
/// Usually used as `Arc<JobHandleInner>`/`JobHandle` which is clonable.
#[derive(Debug)]
pub struct JobHandleInner {
    /// Unique ID of the job.
    job_id: JobId,

    /// Handle to the mpsc channel.
    sender: Option<Sender>,

    /// Tracing span for the future.
    span: Option<Span>,
}

/// A handle to the meta job.
///
/// This handle cannot be cloned, and the wrapper is implemented differently
/// to signal to the monitor when it needs to shut down.
#[derive(Debug)]
pub struct MetaJobHandle {
    /// Unique ID of the job.
    job_id: JobId,

    /// Handle to the mpsc channel.
    sender: Sender,
}

/// Internal metadata of a job.
#[derive(Debug)]
struct JobMetadata {
    job_id: JobId,

    /// Type of the job.
    job_type: JobType,

    /// List of associated nodes.
    ///
    /// Some jobs may be related to multiple nodes (e.g., building
    /// several system profiles at once).
    nodes: Vec<NodeName>,

    /// Current state of this job.
    state: JobState,

    /// Current custom message of this job.
    ///
    /// For jobs in the Failed state, this is the error.
    /// For jobs in the Succeeded state, this might contain a custom
    /// message.
    custom_message: Option<String>,
}

/// Message to create a new job.
#[derive(Debug)]
pub struct JobCreation {
    /// Type of the job.
    job_type: JobType,

    /// List of associated nodes.
    nodes: Vec<NodeName>,
}

/// An event message sent via the mpsc channel.
#[derive(Debug)]
pub struct Event {
    /// Unique ID of the job.
    job_id: JobId,

    /// Event payload.
    payload: EventPayload,
}

/// The payload of an event.
#[derive(Debug)]
pub enum EventPayload {
    /// The job is created.
    Creation(JobCreation),

    /// The job succeeded with a custom message.
    SuccessWithMessage(String),

    /// The job failed.
    ///
    /// We can't pass the ColmenaError because the wrapper needs to
    /// be able to return it as-is.
    Failure(String),

    /// The job was no-op.
    ///
    /// This probably means that some precondition wasn't met and
    /// this job didn't make any changes.
    ///
    /// This puts the job in the Succeeded state but causes the
    /// progress spinner to disappear.
    Noop(String),

    /// The job wants to transition to a new state.
    NewState(JobState),

    /// The child process printed a line to stdout.
    ChildStdout(String),

    /// The child process printed a line to stderr.
    ChildStderr(String),

    /// A normal message from the job itself.
    Message(String),

    /// The monitor should shut down.
    ///
    /// This is sent at the end of the meta job regardless of the outcome.
    ShutdownMonitor,
}

struct JobStats {
    waiting: usize,
    running: usize,
    succeeded: usize,
    failed: usize,
}

impl JobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl JobMonitor {
    /// Creates a new job monitor and a meta job.
    pub fn new(progress: Option<ProgressSender>) -> (Self, MetaJobHandle) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let meta_job_id = JobId::new();

        let mut monitor = Self {
            receiver,
            events: Vec::new(),
            jobs: HashMap::new(),
            meta_job_id,
            progress,
            label_width: None,
        };

        let metadata = JobMetadata {
            job_id: meta_job_id,
            job_type: JobType::Meta,
            nodes: Vec::new(),
            state: JobState::Running,
            custom_message: None,
        };

        monitor.jobs.insert(meta_job_id, metadata);

        let job = MetaJobHandle {
            job_id: meta_job_id,
            sender,
        };

        (monitor, job)
    }

    /// Sets the max label width.
    pub fn set_label_width(&mut self, label_width: usize) {
        self.label_width = Some(label_width);
    }

    /// Starts the monitor.
    pub async fn run_until_completion(mut self) -> ColmenaResult<Self> {
        if let Some(width) = self.label_width
            && let Some(sender) = &self.progress
        {
            sender.send(ProgressMessage::HintLabelWidth(width)).unwrap();
        }

        loop {
            let message = self.receiver.recv().await;

            if message.is_none() {
                // All sending halves have been closed - We are done!
                return self.finish().await;
            }

            let message = message.unwrap();

            match &message.payload {
                EventPayload::Creation(creation) => {
                    let metadata = JobMetadata {
                        job_id: message.job_id,
                        job_type: creation.job_type,
                        nodes: creation.nodes.clone(),
                        state: JobState::Waiting,
                        custom_message: None,
                    };

                    let existing = self.jobs.insert(message.job_id, metadata);
                    assert!(existing.is_none());
                }
                EventPayload::ShutdownMonitor => {
                    // The meta job has returned - We are done!
                    assert_eq!(self.meta_job_id, message.job_id);
                    return self.finish().await;
                }
                EventPayload::NewState(new_state) => {
                    self.update_job_state(message.job_id, *new_state, None, false);

                    if message.job_id != self.meta_job_id {
                        self.print_job_stats();
                    }
                }
                EventPayload::SuccessWithMessage(custom_message) => {
                    let custom_message = Some(custom_message.clone());
                    self.update_job_state(
                        message.job_id,
                        JobState::Succeeded,
                        custom_message,
                        false,
                    );

                    if message.job_id != self.meta_job_id {
                        self.print_job_stats();
                    }
                }
                EventPayload::Noop(custom_message) => {
                    let custom_message = Some(custom_message.clone());
                    self.update_job_state(
                        message.job_id,
                        JobState::Succeeded,
                        custom_message,
                        true,
                    );

                    if message.job_id != self.meta_job_id {
                        self.print_job_stats();
                    }
                }
                EventPayload::Failure(error) => {
                    let error = Some(error.clone());
                    self.update_job_state(message.job_id, JobState::Failed, error, false);

                    if message.job_id != self.meta_job_id {
                        self.print_job_stats();
                    }
                }
                EventPayload::ChildStdout(m)
                | EventPayload::ChildStderr(m)
                | EventPayload::Message(m) => {
                    if let Some(sender) = &self.progress {
                        let metadata = &self.jobs[&message.job_id];
                        let line = metadata.get_line(m.clone());
                        let pm = self.get_print_message(message.job_id, line);
                        sender.send(pm).unwrap();
                    }
                }
            }

            self.events.push(message);
        }
    }

    /// Updates the state of a job.
    fn update_job_state(
        &mut self,
        job_id: JobId,
        new_state: JobState,
        message: Option<String>,
        noop: bool,
    ) {
        let metadata = self.jobs.get_mut(&job_id).unwrap();
        let old_state = metadata.state;

        if old_state == new_state {
            return;
        } else if old_state.is_final() {
            tracing::debug!("Tried to update the state of a finished job");
            return;
        }

        metadata.state = new_state;

        if message.is_some() {
            metadata.custom_message = message;
        }

        if new_state != JobState::Waiting
            && let Some(sender) = &self.progress
        {
            let text = if new_state == JobState::Succeeded {
                metadata
                    .custom_message
                    .clone()
                    .or_else(|| metadata.describe_state_transition())
            } else {
                metadata.describe_state_transition()
            };

            if let Some(text) = text {
                let line = if noop {
                    // Spinner should disappear
                    metadata.get_line(text).style(LineStyle::SuccessNoop)
                } else {
                    metadata.get_line(text)
                };

                let message = self.get_print_message(job_id, line);
                sender.send(message).unwrap();
            }
        }
    }

    /// Updates the user-visible job statistics output.
    fn print_job_stats(&self) {
        if let Some(sender) = &self.progress {
            let stats = self.get_job_stats();
            let text = format!("{}", stats);
            let line = self.jobs[&self.meta_job_id].get_line(text).noisy();
            let message = ProgressMessage::PrintMeta(line);
            sender.send(message).unwrap();
        }
    }

    /// Returns jobs statistics.
    fn get_job_stats(&self) -> JobStats {
        let mut waiting = 0;
        let mut running = 0;
        let mut succeeded = 0;
        let mut failed = 0;

        for job in self.jobs.values() {
            if job.job_id == self.meta_job_id {
                continue;
            }

            match job.state {
                JobState::Waiting => {
                    waiting += 1;
                }
                JobState::Running => {
                    running += 1;
                }
                JobState::Succeeded => {
                    succeeded += 1;
                }
                JobState::Failed => {
                    failed += 1;
                }
            }
        }

        JobStats {
            waiting,
            running,
            succeeded,
            failed,
        }
    }

    fn get_print_message(&self, job_id: JobId, line: Line) -> ProgressMessage {
        if job_id == self.meta_job_id {
            ProgressMessage::PrintMeta(line)
        } else {
            ProgressMessage::Print(line)
        }
    }

    /// Shows human-readable summary and performs cleanup.
    async fn finish(mut self) -> ColmenaResult<Self> {
        if let Some(sender) = self.progress.take() {
            sender.send(ProgressMessage::Complete).unwrap();
        }

        // HACK
        time::sleep(Duration::from_secs(1)).await;

        for job in self.jobs.values() {
            if job.state == JobState::Failed {
                let logs: Vec<&Event> = self
                    .events
                    .iter()
                    .filter(|e| e.job_id == job.job_id)
                    .collect();
                let last_logs: Vec<&Event> = logs
                    .into_iter()
                    .rev()
                    .take(LOG_CONTEXT_LINES)
                    .rev()
                    .collect();

                tracing::error!(
                    "{} - Last {} lines of logs:",
                    job.get_failure_summary(),
                    last_logs.len()
                );
                for event in last_logs {
                    tracing::error!("{}", event.payload);
                }
            }
        }

        Ok(self)
    }
}

impl JobState {
    /// Returns whether this state is final.
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Failed | Self::Succeeded)
    }
}

impl JobHandleInner {
    /// Creates a JobHandle that isn't connected to a JobMonitor.
    pub fn null() -> Self {
        Self {
            job_id: JobId::new(),
            sender: None,
            span: None,
        }
    }

    /// Creates a new job with a distinct ID.
    ///
    /// This sends out a Creation message with the metadata.
    pub fn create_job(&self, job_type: JobType, nodes: Vec<NodeName>) -> ColmenaResult<JobHandle> {
        let job_id = JobId::new();
        let creation = JobCreation { job_type, nodes };

        if job_type == JobType::Meta {
            return Err(ColmenaError::Unknown {
                message: "Cannot create a meta job!".to_string(),
            });
        }

        let new_handle = Arc::new(Self {
            job_id,
            sender: self.sender.clone(),
            span: Some(make_span(job_type)),
        });

        new_handle.send_payload(EventPayload::Creation(creation))?;

        Ok(new_handle)
    }

    /// Runs a closure, automatically updating the job monitor based on the result.
    ///
    /// This immediately transitions the state to Running.
    pub async fn run<F, U, T>(self: Arc<Self>, f: U) -> ColmenaResult<T>
    where
        U: FnOnce(Arc<Self>) -> F,
        F: Future<Output = ColmenaResult<T>>,
    {
        self.run_internal(f, true).await
    }

    /// Runs a closure, automatically updating the job monitor based on the result.
    ///
    /// This does not immediately transition the state to Running.
    pub async fn run_waiting<F, U, T>(self: Arc<Self>, f: U) -> ColmenaResult<T>
    where
        U: FnOnce(Arc<Self>) -> F,
        F: Future<Output = ColmenaResult<T>>,
    {
        self.run_internal(f, false).await
    }

    /// Sends a line of child stdout to the job monitor.
    pub fn stdout(&self, output: String) -> ColmenaResult<()> {
        self.send_payload(EventPayload::ChildStdout(output))
    }

    /// Sends a line of child stderr to the job monitor.
    pub fn stderr(&self, output: String) -> ColmenaResult<()> {
        self.send_payload(EventPayload::ChildStderr(output))
    }

    /// Sends a human-readable message to the job monitor.
    pub fn message(&self, message: String) -> ColmenaResult<()> {
        self.send_payload(EventPayload::Message(message))
    }

    /// Transitions to a new job state.
    pub fn state(&self, new_state: JobState) -> ColmenaResult<()> {
        self.send_payload(EventPayload::NewState(new_state))
    }

    /// Marks the job as successful, with a custom message.
    pub fn success_with_message(&self, message: String) -> ColmenaResult<()> {
        self.send_payload(EventPayload::SuccessWithMessage(message))
    }

    /// Marks the job as noop.
    pub fn noop(&self, message: String) -> ColmenaResult<()> {
        self.send_payload(EventPayload::Noop(message))
    }

    /// Marks the job as failed.
    pub fn failure(&self, error: &ColmenaError) -> ColmenaResult<()> {
        self.send_payload(EventPayload::Failure(error.to_string()))
    }

    /// Runs a closure, automatically updating the job monitor based on the result.
    async fn run_internal<F, U, T>(self: Arc<Self>, f: U, report_running: bool) -> ColmenaResult<T>
    where
        U: FnOnce(Arc<Self>) -> F,
        F: Future<Output = ColmenaResult<T>>,
    {
        if report_running {
            // Tell monitor we are starting
            self.send_payload(EventPayload::NewState(JobState::Running))?;
        }

        let fut = match &self.span {
            Some(span) => Either::Left(f(self.clone()).instrument(span.clone())),
            None => Either::Right(f(self.clone())),
        };

        match fut.await {
            Ok(val) => {
                // Success!
                self.state(JobState::Succeeded)?;

                Ok(val)
            }
            Err(e) => {
                self.failure(&e)?;

                Err(e)
            }
        }
    }

    /// Sends an event to the job monitor.
    fn send_payload(&self, payload: EventPayload) -> ColmenaResult<()> {
        if payload.privileged() {
            panic!("Tried to send privileged payload with JobHandle");
        }

        let event = Event::new(self.job_id, payload);

        if let Some(sender) = &self.sender {
            sender
                .send(event)
                .map_err(|e| ColmenaError::unknown(Box::new(e)))?;
        } else {
            tracing::debug!("Sending event: {:?}", event);
        }

        Ok(())
    }
}

impl MetaJobHandle {
    /// Runs a closure, automatically updating the job monitor based on the result.
    pub async fn run<F, U, T>(self, f: U) -> ColmenaResult<T>
    where
        U: FnOnce(JobHandle) -> F,
        F: Future<Output = ColmenaResult<T>>,
    {
        let normal_handle = Arc::new(JobHandleInner {
            job_id: self.job_id,
            sender: Some(self.sender.clone()),
            span: Some(make_span(JobType::Meta)),
        });

        match f(normal_handle).await {
            Ok(val) => {
                self.send_payload(EventPayload::NewState(JobState::Succeeded))?;
                self.send_payload(EventPayload::ShutdownMonitor)?;

                Ok(val)
            }
            Err(e) => {
                self.send_payload(EventPayload::Failure(e.to_string()))?;
                self.send_payload(EventPayload::ShutdownMonitor)?;

                Err(e)
            }
        }
    }

    /// Sends an event to the job monitor.
    fn send_payload(&self, payload: EventPayload) -> ColmenaResult<()> {
        let event = Event::new(self.job_id, payload);

        self.sender
            .send(event)
            .map_err(|e| ColmenaError::unknown(Box::new(e)))?;

        Ok(())
    }
}

impl JobMetadata {
    /// Returns a short human-readable label.
    fn get_label(&self) -> &str {
        if self.job_type == JobType::Meta {
            ""
        } else if self.nodes.len() != 1 {
            "(...)"
        } else {
            self.nodes[0].as_str()
        }
    }

    /// Returns a Line struct with the given text.
    fn get_line(&self, text: String) -> Line {
        let style = match self.state {
            JobState::Succeeded => LineStyle::Success,
            JobState::Failed => LineStyle::Failure,
            _ => LineStyle::Normal,
        };

        Line::new(self.job_id, text)
            .style(style)
            .label(self.get_label().to_string())
    }

    /// Returns a human-readable string describing the transition to the current state.
    fn describe_state_transition(&self) -> Option<String> {
        if self.state == JobState::Waiting {
            return None;
        }

        let node_list =
            describe_node_list(&self.nodes).unwrap_or_else(|| "some node(s)".to_string());

        let message = self.custom_message.as_deref().unwrap_or("No message");

        Some(match (self.job_type, self.state) {
            (JobType::Meta, JobState::Succeeded) => "All done!".to_string(),

            (JobType::Evaluate, JobState::Running) => format!("Evaluating {}", node_list),
            (JobType::Evaluate, JobState::Succeeded) => format!("Evaluated {}", node_list),
            (JobType::Evaluate, JobState::Failed) => format!("Evaluation failed: {}", message),

            (JobType::Build, JobState::Running) => format!("Building {}", node_list),
            (JobType::Build, JobState::Succeeded) => format!("Built {}", node_list),
            (JobType::Build, JobState::Failed) => format!("Build failed: {}", message),

            (JobType::Push, JobState::Running) => "Pushing system closure".to_string(),
            (JobType::Push, JobState::Succeeded) => "Pushed system closure".to_string(),
            (JobType::Push, JobState::Failed) => format!("Push failed: {}", message),

            (JobType::UploadKeys, JobState::Running) => "Uploading keys".to_string(),
            (JobType::UploadKeys, JobState::Succeeded) => "Uploaded keys".to_string(),
            (JobType::UploadKeys, JobState::Failed) => format!("Key upload failed: {}", message),

            (JobType::Activate, JobState::Running) => "Activating system profile".to_string(),
            (JobType::Activate, JobState::Failed) => format!("Activation failed: {}", message),

            (JobType::Reboot, JobState::Running) => "Rebooting".to_string(),
            (JobType::Reboot, JobState::Succeeded) => "Rebooted".to_string(),
            (JobType::Reboot, JobState::Failed) => format!("Reboot failed: {}", message),

            (_, JobState::Failed) => format!("Failed: {}", message),
            (_, JobState::Succeeded) => "Succeeded".to_string(),
            _ => "".to_string(),
        })
    }

    /// Returns a human-readable string describing a failed job for use in the summary.
    fn get_failure_summary(&self) -> String {
        let node_list =
            describe_node_list(&self.nodes).unwrap_or_else(|| "some node(s)".to_string());

        match self.job_type {
            JobType::Evaluate => format!("Failed to evaluate {}", node_list),
            JobType::Build => format!("Failed to build {}", node_list),
            JobType::Push => format!("Failed to push system closure to {}", node_list),
            JobType::UploadKeys => format!("Failed to upload keys to {}", node_list),
            JobType::Activate => format!("Failed to deploy to {}", node_list),
            JobType::Reboot => format!("Failed to reboot {}", node_list),
            JobType::Meta => "Failed to complete requested operation".to_string(),
            _ => format!("Failed to complete job on {}", node_list),
        }
    }
}

impl Event {
    /// Creates a new event.
    fn new(job_id: JobId, payload: EventPayload) -> Self {
        Self { job_id, payload }
    }
}

impl EventPayload {
    fn privileged(&self) -> bool {
        matches!(self, Self::ShutdownMonitor)
    }
}

impl Display for EventPayload {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EventPayload::ChildStdout(o) => write!(f, "  stdout) {}", o)?,
            EventPayload::ChildStderr(o) => write!(f, "  stderr) {}", o)?,
            EventPayload::Message(m) => write!(f, " message) {}", m)?,
            EventPayload::Creation(_) => write!(f, " created)")?,
            EventPayload::NewState(s) => write!(f, "   state) {:?}", s)?,
            EventPayload::SuccessWithMessage(m) => write!(f, " success) {}", m)?,
            EventPayload::Noop(m) => write!(f, "    noop) {}", m)?,
            EventPayload::Failure(e) => write!(f, " failure) {}", e)?,
            EventPayload::ShutdownMonitor => write!(f, "shutdown)")?,
        }

        Ok(())
    }
}

impl Display for JobStats {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut first = true;
        fn comma(f: &mut fmt::Formatter, first: &mut bool) -> fmt::Result {
            if *first {
                *first = false;
                return Ok(());
            }
            write!(f, ", ")
        }

        if self.running != 0 {
            comma(f, &mut first)?;
            write!(f, "{} running", self.running)?;
        }

        if self.succeeded != 0 {
            comma(f, &mut first)?;
            write!(f, "{} succeeded", self.succeeded)?;
        }

        if self.failed != 0 {
            comma(f, &mut first)?;
            write!(f, "{} failed", self.failed)?;
        }

        if self.waiting != 0 {
            comma(f, &mut first)?;
            write!(f, "{} waiting", self.waiting)?;
        }

        Ok(())
    }
}

/// Returns a JobHandle that is not connected to a JobMonitor.
pub fn null_job_handle() -> JobHandle {
    Arc::new(JobHandleInner::null())
}

/// Returns a textual description of a list of nodes.
///
/// Example: "alpha, beta, and 5 other nodes"
fn describe_node_list(nodes: &[NodeName]) -> Option<String> {
    let rough_limit = 40;
    let other_text = ", and XX other nodes";

    let total = nodes.len();
    if total == 0 {
        return None;
    }

    let mut s = String::new();
    let mut iter = nodes.iter().enumerate().peekable();

    while let Some((_, node)) = iter.next() {
        let next = iter.peek();

        if !s.is_empty() {
            if next.is_none() {
                s += if total > 2 { ", and " } else { " and " };
            } else {
                s += ", "
            }
        }

        s += node.as_str();

        if next.is_none() {
            break;
        }

        let (idx, next) = next.unwrap();
        let remaining_text = rough_limit - s.len();
        let remaining_nodes = total - idx;

        if next.len() + other_text.len() >= remaining_text {
            if remaining_nodes == 1 {
                write!(s, ", and {}", next.as_str()).unwrap();
                break;
            } else {
                write!(s, ", and {} other nodes", remaining_nodes).unwrap();
                break;
            }
        }
    }

    Some(s)
}

fn make_span(job_type: JobType) -> Span {
    let job_type_str = match job_type {
        JobType::Meta => "meta",
        JobType::Evaluate => "evaluate",
        JobType::Build => "build",
        JobType::UploadKeys => "upload-keys",
        JobType::Push => "push",
        JobType::Activate => "activate",
        JobType::Execute => "execute",
        JobType::CreateGcRoots => "create-gc-roots",
        JobType::Reboot => "reboot",
    };
    tracing::info_span!("job", job_type = job_type_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio_test::block_on;

    macro_rules! node {
        ($n:expr) => {
            NodeName::new($n.to_string()).unwrap()
        };
    }

    #[test]
    fn test_monitor_event() {
        block_on(async {
            let (monitor, meta) = JobMonitor::new(None);

            let meta = meta.run(|job: JobHandle| async move {
                job.message("hello world".to_string())?;

                let eval_job = job.create_job(JobType::Evaluate, vec![node!("alpha")])?;
                eval_job
                    .run(|job| async move {
                        job.stdout("child stdout".to_string())?;

                        Ok(())
                    })
                    .await?;

                Err(ColmenaError::Unsupported) as ColmenaResult<()>
            });

            // Run until completion
            let (ret, monitor) = tokio::join!(meta, monitor.run_until_completion(),);

            match ret {
                Err(ColmenaError::Unsupported) => (),
                _ => {
                    panic!("Wrapper must return error as-is");
                }
            }

            let monitor = monitor.unwrap();

            assert_eq!(2, monitor.jobs.len());

            for event in monitor.events.iter() {
                match &event.payload {
                    EventPayload::Message(m) => {
                        assert_eq!("hello world", m);
                    }
                    EventPayload::ChildStdout(m) => {
                        assert_eq!("child stdout", m);
                    }
                    _ => {}
                }
            }
        });
    }
}
