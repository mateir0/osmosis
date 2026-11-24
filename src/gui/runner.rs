//! Job plumbing: the messages a worker thread sends to the UI, the summary a
//! finished job leaves behind, and the `Runner` implementation that forwards
//! `RunnerEvent`s from the setup/games engines into the UI channel.

use crate::setup::{CommandOutput, Runner, RunnerEvent, SystemRunner};
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Scan,
    Stage,
    Import,
    Setup,
    Games,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            JobKind::Scan => "scan",
            JobKind::Stage => "stage",
            JobKind::Import => "import",
            JobKind::Setup => "setup",
            JobKind::Games => "games",
        }
    }
}

/// One line of progress from a running job, or its final outcome.
#[derive(Debug)]
pub enum JobMsg {
    Event(String),
    Done(Result<JobSummary, String>),
}

/// What a finished job leaves in the UI: the report summary text the engine
/// already builds, plus the exit-code convention the CLI uses.
#[derive(Debug, Clone)]
pub struct JobSummary {
    pub kind: JobKind,
    pub ok: bool,
    pub exit_code: u8,
    pub text: String,
}

/// Runs the real commands through `SystemRunner` (same argv, same
/// DEBIAN_FRONTEND passthrough as the CLI) and mirrors every engine progress
/// event into the UI channel.
pub struct GuiRunner {
    pub tx: Sender<JobMsg>,
}

impl Runner for GuiRunner {
    fn output(&self, argv: &[&str]) -> Result<CommandOutput, String> {
        SystemRunner.output(argv)
    }

    fn on_event(&self, event: &RunnerEvent) {
        let line = match event {
            RunnerEvent::Start { title, .. } => format!(">> {}", title),
            RunnerEvent::Finish { id, status, detail } => {
                format!("<< {} [{}] {}", id, status, detail)
            }
            RunnerEvent::Command { argv } => format!("$ {}", argv.join(" ")),
        };
        let _ = self.tx.send(JobMsg::Event(line));
    }
}
