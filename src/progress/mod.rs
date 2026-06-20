//! Progress output.
//!
//! Displaying of progress is handled through a ProgressOutput. Each
//! ProgressOutput is minimally-stateful and receives already formatted
//! text from a writer (e.g., a JobMonitor).

pub mod plain;
pub mod spinner;

use std::io::IsTerminal;

use tokio::sync::mpsc::{self, UnboundedReceiver as TokioReceiver, UnboundedSender as TokioSender};

use crate::error::ColmenaResult;
use crate::job::JobId;

pub use plain::PlainOutput;
pub use spinner::SpinnerOutput;

pub type Sender = TokioSender<Message>;
pub type Receiver = TokioReceiver<Message>;

const DEFAULT_LABEL_WIDTH: usize = 5;

pub enum SimpleProgressOutput {
    Plain(PlainOutput),
    Spinner(SpinnerOutput),
}

/// A progress display driver.
pub trait ProgressOutput: Sized {
    /// Runs until a Message::Complete is received.
    async fn run_until_completion(self) -> ColmenaResult<Self>;

    /// Returns a sender.
    ///
    /// This method can only be called once.
    fn get_sender(&mut self) -> Option<Sender>;
}

/// A message.
#[derive(Debug, Clone)]
pub enum Message {
    /// Prints a line of text to the screen.
    Print(Line),

    /// Prints a line of text related to the overall progress.
    ///
    /// For certain output types, this will be printed in a fixed,
    /// prominent position with special styling.
    PrintMeta(Line),

    /// Hints about the maximum label width.
    HintLabelWidth(usize),

    /// Completes the progress output.
    Complete,
}

/// A line of output.
#[derive(Debug, Clone)]
pub struct Line {
    /// Identifier for elapsed time tracking.
    job_id: JobId,

    /// Style of the line.
    style: LineStyle,

    /// A label.
    label: String,

    /// The text.
    text: String,

    /// Whether this is an one-off output.
    one_off: bool,

    /// Whether this is line is noisy.
    noisy: bool,
}

/// Style of a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStyle {
    Normal,
    Success,
    SuccessNoop,
    Failure,
}

impl SimpleProgressOutput {
    pub fn new(verbose: bool) -> Self {
        let tty = std::io::stdout().is_terminal();

        if verbose || !tty {
            Self::Plain(PlainOutput::new())
        } else {
            Self::Spinner(SpinnerOutput::new())
        }
    }

    pub fn get_sender(&mut self) -> Option<Sender> {
        match self {
            Self::Plain(ref mut o) => o.get_sender(),
            Self::Spinner(ref mut o) => o.get_sender(),
        }
    }

    pub async fn run_until_completion(self) -> ColmenaResult<Self> {
        match self {
            Self::Plain(o) => o.run_until_completion().await.map(Self::Plain),
            Self::Spinner(o) => o.run_until_completion().await.map(Self::Spinner),
        }
    }
}

impl Line {
    pub fn new(job_id: JobId, text: String) -> Self {
        Self {
            job_id,
            style: LineStyle::Normal,
            label: String::new(),
            text,
            one_off: false,
            noisy: false,
        }
    }

    /// Builder-like interface to set the line as noisy.
    pub fn noisy(mut self) -> Self {
        self.noisy = true;
        self
    }

    /// Builder-like interface to set the label.
    pub fn label(mut self, label: String) -> Self {
        self.label = label;
        self
    }

    /// Builder-like interface to set the line style.
    pub fn style(mut self, style: LineStyle) -> Self {
        self.style = style;
        self
    }
}

fn create_channel() -> (Sender, Receiver) {
    mpsc::unbounded_channel()
}
