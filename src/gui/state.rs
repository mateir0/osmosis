//! Application state: one struct owned by the single egui thread. Worker
//! threads only touch it through the job channel.

use crate::gui::jobs::JobHandle;
use crate::gui::nav::Screen;
use crate::gui::runner::{JobMsg, JobSummary};
use crate::preflight::PreflightReport;
use std::sync::{Arc, Mutex};

const MAX_EVENTS: usize = 600;

pub struct App {
    pub screen: Screen,
    pub dry_run: bool,
    // preflight
    pub preflight_manifest: String,
    pub preflight_out: String,
    pub preflight_shared: Arc<Mutex<Option<PreflightReport>>>,
    // scan
    pub scan_root: String,
    pub scan_out: String,
    pub scan_staging: String,
    pub scan_config: String,
    // stage
    pub stage_manifest: String,
    pub stage_staging: String,
    // import / setup / games
    pub import_stage: String,
    pub import_home: String,
    pub setup_home: String,
    pub games_root: String,
    pub games_home: String,
    // job plumbing
    pub job: Option<JobHandle>,
    pub events: Vec<String>,
    pub result: Option<JobSummary>,
    pub error: Option<String>,
    egui_ctx: Option<egui::Context>,
}

fn env_or(var: &str, fallback: &str) -> String {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| fallback.to_string())
}

impl App {
    pub fn new() -> App {
        let home = env_or("HOME", "");
        let profile = if cfg!(windows) {
            env_or("USERPROFILE", &home)
        } else {
            home.clone()
        };
        App {
            screen: crate::gui::nav::screens_for_platform()[0],
            dry_run: false,
            preflight_manifest: "./osmosis-output/manifest.json".to_string(),
            preflight_out: "./osmosis-output".to_string(),
            preflight_shared: Arc::new(Mutex::new(None)),
            scan_root: profile,
            scan_out: "./osmosis-output".to_string(),
            scan_staging: String::new(),
            scan_config: String::new(),
            stage_manifest: "./osmosis-output/manifest.json".to_string(),
            stage_staging: String::new(),
            import_stage: String::new(),
            import_home: home.clone(),
            setup_home: home.clone(),
            games_root: String::new(),
            games_home: home,
            job: None,
            events: Vec::new(),
            result: None,
            error: None,
            egui_ctx: None,
        }
    }

    pub fn set_ctx(&mut self, ctx: &egui::Context) {
        self.egui_ctx = Some(ctx.clone());
    }

    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// The last finished job's (kind, ok) — drives the Next button.
    pub fn last_ok(&self) -> Option<(crate::gui::runner::JobKind, bool)> {
        self.result.as_ref().map(|r| (r.kind, r.ok))
    }

    /// Arm a new job, clearing the previous run's log/result/error.
    pub fn start_job(&mut self, handle: JobHandle) {
        self.events.clear();
        self.result = None;
        self.error = None;
        self.job = Some(handle);
    }

    /// Drain pending messages from the running job; repaint while it runs.
    pub fn poll_job(&mut self) {
        let Some(job) = self.job.take() else {
            return;
        };
        let mut still_running = true;
        let mut got_any = false;
        while let Ok(msg) = job.rx.try_recv() {
            got_any = true;
            match msg {
                JobMsg::Event(line) => {
                    self.events.push(line);
                    if self.events.len() > MAX_EVENTS {
                        let overflow = self.events.len() - MAX_EVENTS;
                        self.events.drain(..overflow);
                    }
                }
                JobMsg::Done(Ok(summary)) => {
                    self.result = Some(summary);
                    still_running = false;
                }
                JobMsg::Done(Err(e)) => {
                    self.error = Some(e);
                    still_running = false;
                }
            }
        }
        if still_running {
            self.job = Some(job);
        }
        if got_any {
            if let Some(ctx) = &self.egui_ctx {
                ctx.request_repaint();
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
