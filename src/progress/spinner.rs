//! Progress spinner output.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use super::{
    create_channel, Line, LineStyle, Message, ProgressOutput, Receiver, Sender, DEFAULT_LABEL_WIDTH,
};
use crate::error::ColmenaResult;
use crate::job::JobId;

/// Progress spinner output.
pub struct SpinnerOutput {
    /// Job timekeeping.
    job_state: HashMap<JobId, JobState>,

    /// One-off progress bars.
    one_off_bars: Vec<(ProgressBar, LineStyle)>,

    /// Progress bar for the meta job.
    meta_bar: ProgressBar,

    /// Last style printed to the meta bar.
    meta_style: LineStyle,

    /// Maximum label width for alignment.
    label_width: usize,

    multi: MultiProgress,
    sender: Option<Sender>,
    receiver: Receiver,
}

#[derive(Clone)]
struct JobState {
    /// When the job started.
    since: Instant,

    /// Progress bar to draw to.
    bar: ProgressBar,

    /// Last style printed to the bar.
    ///
    /// This is used to regenerate the approproate style when the
    /// max label width changes.
    style: LineStyle,
}

impl SpinnerOutput {
    pub fn new() -> Self {
        let meta_bar = {
            ProgressBar::new(100)
                .with_style(get_spinner_style(DEFAULT_LABEL_WIDTH, LineStyle::Normal))
        };

        let (sender, receiver) = create_channel();

        Self {
            multi: MultiProgress::new(),
            job_state: HashMap::new(),
            one_off_bars: Vec::new(),
            meta_bar,
            meta_style: LineStyle::Normal,
            label_width: DEFAULT_LABEL_WIDTH,
            sender: Some(sender),
            receiver,
        }
    }

    /// Returns the state of a job.
    fn get_job_state(&mut self, job_id: JobId) -> JobState {
        if let Some(state) = self.job_state.get(&job_id) {
            state.clone()
        } else {
            let bar = self.create_bar(LineStyle::Normal);
            let state = JobState::new(bar);
            self.job_state.insert(job_id, state.clone());
            state
        }
    }

    /// Creates a new bar.
    fn create_bar(&self, style: LineStyle) -> ProgressBar {
        let bar = ProgressBar::new(100).with_style(self.get_spinner_style(style));

        let bar = self.multi.add(bar);
        bar.enable_steady_tick(Duration::from_millis(100));
        bar
    }

    fn print(&mut self, line: Line, meta: bool) {
        if line.label.len() > self.label_width {
            self.label_width = line.label.len();
            self.reset_styles();
        }

        let bar = if meta {
            if self.meta_style != line.style {
                self.meta_style = line.style;
                self.meta_bar.set_style(self.get_spinner_style(line.style));
            }

            self.meta_bar.clone()
        } else {
            let mut state = self.get_job_state(line.job_id);

            if line.one_off {
                let bar = self.create_bar(line.style);
                state.configure_one_off(&bar);
                self.one_off_bars.push((bar.clone(), line.style));
                bar
            } else {
                let bar = state.bar.clone();

                if state.style != line.style {
                    state.style = line.style;
                    bar.set_style(self.get_spinner_style(line.style));
                    self.job_state.insert(line.job_id, state);
                }

                bar
            }
        };

        bar.set_prefix(line.label);

        match line.style {
            LineStyle::Success | LineStyle::Failure => {
                bar.finish_with_message(line.text);
            }
            LineStyle::SuccessNoop => {
                bar.finish_and_clear();
            }
            _ => {
                bar.set_message(line.text);
            }
        }
    }

    /// Resets the styles of all known bars.
    fn reset_styles(&self) {
        for (bar, style) in &self.one_off_bars {
            let style = self.get_spinner_style(*style);
            bar.set_style(style);
        }

        for state in self.job_state.values() {
            let style = self.get_spinner_style(state.style);
            state.bar.set_style(style);
        }

        let style = self.get_spinner_style(self.meta_style);
        self.meta_bar.set_style(style);
    }

    fn get_spinner_style(&self, style: LineStyle) -> ProgressStyle {
        get_spinner_style(self.label_width, style)
    }
}

impl ProgressOutput for SpinnerOutput {
    async fn run_until_completion(mut self) -> ColmenaResult<Self> {
        let meta_bar = self.multi.add(self.meta_bar.clone());
        meta_bar.enable_steady_tick(Duration::from_millis(100));

        loop {
            let message = self.receiver.recv().await;

            if message.is_none() {
                return Ok(self);
            }

            let message = message.unwrap();

            match message {
                Message::Complete => {
                    // To work around https://github.com/console-rs/indicatif/issues/342
                    println!();

                    return Ok(self);
                }
                Message::Print(line) => {
                    self.print(line, false);
                }
                Message::PrintMeta(line) => {
                    self.print(line, true);
                }
                Message::HintLabelWidth(width) => {
                    if width > self.label_width {
                        self.label_width = width;
                        self.reset_styles();
                    }
                }
            }
        }
    }

    fn get_sender(&mut self) -> Option<Sender> {
        self.sender.take()
    }
}

impl JobState {
    fn new(bar: ProgressBar) -> Self {
        Self {
            since: Instant::now(),
            bar,
            style: LineStyle::Normal,
        }
    }

    fn configure_one_off(&self, bar: &ProgressBar) {
        bar.clone()
            .with_elapsed(Instant::now().duration_since(self.since));
    }
}

fn get_spinner_style(label_width: usize, style: LineStyle) -> ProgressStyle {
    let template = format!(
        "{{prefix:>{}.bold.dim}} {{spinner}} {{elapsed}} {{wide_msg}}",
        label_width
    );

    match style {
        LineStyle::Normal | LineStyle::Success | LineStyle::SuccessNoop => {
            ProgressStyle::default_spinner()
                .tick_chars("🕛🕐🕑🕒🕓🕔🕕🕖🕗🕘🕙🕚✅")
                .template(&template)
                .unwrap()
        }
        LineStyle::Failure => ProgressStyle::default_spinner()
            .tick_chars("❌❌")
            .template(&template)
            .unwrap(),
    }
}
