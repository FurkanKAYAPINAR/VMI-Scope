//! `vmiscope --bench`: the perf harness for task 7.10.
//!
//! Three scenarios, each an amount of data the plan named as a target: a
//! 50,000-row table, a 1,400-class list, and a 2,000-event stream. The harness
//! fills the real application state with synthetic data, forces the real view
//! on screen, drives it for a fixed number of frames, and prints what it
//! measured.
//!
//! # Why it is a mode of the binary and not an `examples/` target
//!
//! Every widget it has to exercise -- `DataTable`, the class list, the event
//! log -- is `pub(crate)`. An example is a separate crate, so it could only
//! reach them through a public API this crate deliberately does not have, and
//! what it measured would be a *reimplementation* of the table rather than the
//! table. Measuring the thing under test beat keeping the harness out of the
//! shipped binary; the cost is one flag and this module.
//!
//! # What the two numbers mean
//!
//! * **CPU/frame** is `eframe::Frame::info().cpu_usage`: the seconds the last
//!   frame spent in `egui` plus painting. This is the number that answers "is
//!   the UI fast enough", because it is the part the code controls.
//! * **Frame interval** is wall time between successive `ui()` calls. It is
//!   **vsync-bound** -- with a present mode that waits for the display, it will
//!   read ~16.7 ms on a 60 Hz panel whatever the CPU cost is, so on its own it
//!   proves only that the loop is not *slower* than the display. Both are
//!   printed because either alone is misleading.
//!
//! The first frames of a scenario are discarded: they include the font atlas
//! growing, the first tessellation of a new layout, and `egui_extras` measuring
//! its columns. Reporting them as steady state would be reporting the cost of
//! arriving rather than the cost of being there.

use std::time::Instant;

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::views::nav::View;

/// The flag that turns this on.
pub(crate) const FLAG: &str = "--bench";

/// Frames driven per scenario.
const FRAMES: usize = 240;

/// Frames discarded at the start of each scenario. Generous on purpose: the
/// atlas and the column widths both settle inside the first handful, and a
/// benchmark that flatters itself is worse than no benchmark.
const WARMUP: usize = 60;

/// Rows in the table scenario.
const TABLE_ROWS: usize = 50_000;
/// Classes in the class-list scenario. `root\CIMV2` has about this many.
const CLASS_COUNT: usize = 1_400;
/// Events in the stream scenario.
const EVENT_COUNT: usize = 2_000;

/// How many of those have already exited, so the per-row alpha, the exit-status
/// colouring and the dimmed-row path are all exercised rather than skipped.
const EVENTS_ENDED: usize = 1_400;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scenario {
    Table,
    Classes,
    Events,
}

impl Scenario {
    const ALL: [Scenario; 3] = [Scenario::Table, Scenario::Classes, Scenario::Events];

    fn title(self) -> &'static str {
        match self {
            Scenario::Table => "50,000-row result table (Query)",
            Scenario::Classes => "1,400-class list (Explorer)",
            // The plan named the Events view for this one. The harness drives
            // the Process view instead, and the reason is a boundary rather
            // than a preference: `EventLog::push` and `EventRow` are private to
            // `views::events`, which this pass does not own, so a synthetic
            // event stream cannot be installed there from here. The Process
            // view is the same shape of workload -- a capped log of arrived
            // events in a virtualised table -- and strictly the heavier one,
            // because every row also computes a fade alpha, a lifetime and a
            // filter match per frame.
            Scenario::Events => "2,000-event stream (Process)",
        }
    }

    fn view(self) -> View {
        match self {
            Scenario::Table => View::Query,
            Scenario::Classes => View::Explorer,
            Scenario::Events => View::Process,
        }
    }
}

/// One scenario's measurements.
struct Report {
    scenario: Scenario,
    cpu_ms: Stats,
    interval_ms: Stats,
    samples: usize,
}

/// The shape of a set of frame timings.
///
/// Mean *and* the tail. A mean alone hides the thing that is actually felt: one
/// 90 ms frame in a second of 4 ms frames reads as 5.6 ms mean and as a visible
/// hitch.
struct Stats {
    mean: f64,
    p50: f64,
    p95: f64,
    max: f64,
}

impl Stats {
    fn of(mut values: Vec<f64>) -> Self {
        if values.is_empty() {
            return Self {
                mean: 0.0,
                p50: 0.0,
                p95: 0.0,
                max: 0.0,
            };
        }
        values.sort_by(f64::total_cmp);
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
        Self {
            mean,
            p50: at(0.50),
            p95: at(0.95),
            max: *values.last().expect("non-empty"),
        }
    }
}

/// The harness's own state, hung off the app when `--bench` is passed.
pub(crate) struct Bench {
    at: usize,
    frame: usize,
    loaded: bool,
    last: Option<Instant>,
    cpu: Vec<f64>,
    intervals: Vec<f64>,
    reports: Vec<Report>,
}

impl Bench {
    pub(crate) fn new() -> Self {
        Self {
            at: 0,
            frame: 0,
            loaded: false,
            last: None,
            cpu: Vec::with_capacity(FRAMES),
            intervals: Vec::with_capacity(FRAMES),
            reports: Vec::new(),
        }
    }
}

impl VmiScopeApp {
    /// Drive one benchmark frame. Returns false once every scenario is done, at
    /// which point the caller has already been sent a close command.
    ///
    /// Called at the very top of `eframe::App::ui`, before the shell, so the
    /// data is in place for the frame that is about to be drawn.
    pub(crate) fn bench_frame(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(bench) = self.bench.as_mut() else {
            return;
        };
        let Some(&scenario) = Scenario::ALL.get(bench.at) else {
            return;
        };

        // A benchmark that let egui go idle would be measuring how long nothing
        // takes.
        ctx.request_repaint();

        if !bench.loaded {
            bench.loaded = true;
            bench.frame = 0;
            bench.last = None;
            bench.cpu.clear();
            bench.intervals.clear();
            self.view = scenario.view();
            self.bench_load(scenario);
            println!("  running {} ...", scenario.title());
            return;
        }

        let now = Instant::now();
        let bench = self.bench.as_mut().expect("still benching");
        if let Some(previous) = bench.last.replace(now) {
            if bench.frame >= WARMUP {
                bench
                    .intervals
                    .push(now.duration_since(previous).as_secs_f64() * 1000.0);
                // `cpu_usage` is the PREVIOUS frame's cost, which is exactly the
                // frame whose interval was just recorded.
                if let Some(cpu) = frame.info().cpu_usage {
                    bench.cpu.push(f64::from(cpu) * 1000.0);
                }
            }
        }
        bench.frame += 1;

        if bench.frame < FRAMES {
            return;
        }

        let report = Report {
            scenario,
            samples: bench.intervals.len(),
            cpu_ms: Stats::of(std::mem::take(&mut bench.cpu)),
            interval_ms: Stats::of(std::mem::take(&mut bench.intervals)),
        };
        bench.reports.push(report);
        bench.at += 1;
        bench.loaded = false;

        if bench.at >= Scenario::ALL.len() {
            self.bench_finish();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Fill the app with one scenario's synthetic data.
    ///
    /// Synthetic and *varied*: 50,000 identical rows would let every galley come
    /// out of egui's layout cache and would measure a cache hit rate rather than
    /// a table. Each row carries its own index in several columns.
    fn bench_load(&mut self, scenario: Scenario) {
        match scenario {
            Scenario::Table => {
                let columns: Vec<String> = [
                    "Name",
                    "ProcessId",
                    "ExecutablePath",
                    "CommandLine",
                    "WorkingSetSize",
                    "HandleCount",
                    "ThreadCount",
                    "CreationDate",
                ]
                .iter()
                .map(|s| (*s).to_string())
                .collect();
                let rows: Vec<Vec<String>> = (0..TABLE_ROWS)
                    .map(|i| {
                        vec![
                            format!("process_{i}.exe"),
                            i.to_string(),
                            format!("C:\\Windows\\System32\\process_{i}.exe"),
                            format!("\"C:\\Windows\\System32\\process_{i}.exe\" --serve {i}"),
                            (i * 4_096).to_string(),
                            (i % 900).to_string(),
                            (i % 64).to_string(),
                            format!("2026080213{:04}.000000+000", i % 6000),
                        ]
                    })
                    .collect();
                self.result = Some(vmiscope_core::QueryResult {
                    columns,
                    rows,
                    key_columns: Vec::new(),
                    connect_ms: 0,
                    elapsed_ms: 0,
                    completion: vmiscope_core::Completion::Complete,
                });
                self.result_wql = format!("SELECT * FROM Win32_Process /* {TABLE_ROWS} rows */");
                self.query_loading = false;
            }
            Scenario::Classes => {
                // Names that sort and filter like real ones: a mix of prefixes,
                // so the facet chips and the text filter both have work to do.
                let classes: Vec<vmiscope_core::ClassBrief> = (0..CLASS_COUNT)
                    .map(|i| vmiscope_core::ClassBrief {
                        name: match i % 4 {
                            0 => format!("Win32_PerfFormattedData_Counters_Thing{i}"),
                            1 => format!("CIM_Managed{i}Element"),
                            2 => format!("__Class{i}Event"),
                            _ => format!("MSFT_Net{i}Adapter"),
                        },
                        kind: match i % 4 {
                            0 => vmiscope_core::ClassKind::DYNAMIC | vmiscope_core::ClassKind::PERF,
                            1 => vmiscope_core::ClassKind::ABSTRACT,
                            2 => vmiscope_core::ClassKind::SYSTEM | vmiscope_core::ClassKind::EVENT,
                            _ => vmiscope_core::ClassKind::DYNAMIC,
                        },
                        provider: (i % 3 == 0).then(|| "CIMWin32".to_string()),
                    })
                    .collect();
                self.classes = classes.clone();
                self.classes_ns = self.active_ns.clone();
                self.class_cache.insert(self.active_ns.clone(), classes);
                self.classes_loading = false;
                // System classes shown, so the list is the full 1,400 rather
                // than a quarter of it.
                self.config.show_system_classes = true;
            }
            Scenario::Events => self.bench_load_process_stream(),
        }
    }

    /// 2,000 process events folded through the real [`crate::state::processes`]
    /// model, so the rows the table renders are the rows the monitor would have
    /// produced -- including the enrichment states, which decide what the
    /// command-line cell has to draw.
    fn bench_load_process_stream(&mut self) {
        use vmiscope_core::{Enrichment, ProcEvent, ProcInfo, ProcKind};

        // A plausible FILETIME so the Time column is on its wall-clock path
        // rather than its fallback: the fallback is the cheaper of the two, and
        // benchmarking the cheap branch would be flattering the result.
        const BASE_FILETIME: u64 = 116_444_736_000_000_000 + 1_754_000_000 * 10_000_000;

        let event = |kind, i: usize| ProcEvent {
            kind,
            pid: 1_000 + i as u32,
            parent_pid: if i.is_multiple_of(7) {
                4
            } else {
                1_000 + (i / 7) as u32
            },
            name: format!("worker_{i}.exe"),
            session_id: (i % 3) as u32,
            sid: Vec::new(),
            time_created: BASE_FILETIME + (i as u64) * 10_000_000,
            exit_status: i.is_multiple_of(11).then_some(0xc000_0005),
        };

        for i in 0..EVENT_COUNT {
            let at = i as f64 * 0.01;
            self.proc_bench_apply(i as u64, &event(ProcKind::Start, i), at);
            // A spread of enrichment states: found with a command line, found
            // with a NULL one, and never asked. All three render differently.
            let enrichment = match i % 3 {
                0 => Enrichment::Found(ProcInfo {
                    command_line: format!("worker_{i}.exe --shard {i} --verbose"),
                    executable_path: format!("C:\\Program Files\\Thing\\worker_{i}.exe"),
                }),
                1 => Enrichment::Found(ProcInfo::default()),
                _ => Enrichment::Unavailable,
            };
            self.proc_bench_attach(i as u64, format!("CORP\\user{}", i % 40), enrichment);
            if i < EVENTS_ENDED {
                self.proc_bench_apply(
                    (EVENT_COUNT + i) as u64,
                    &event(ProcKind::Stop, i),
                    at + 0.5,
                );
            }
        }
    }

    fn bench_finish(&mut self) {
        let Some(bench) = self.bench.as_ref() else {
            return;
        };
        let mut out = String::new();
        let mut line = |text: String| {
            out.push_str(&text);
            out.push('\n');
        };

        line("VMI-Scope perf benchmark (task 7.10)".into());
        line(format!(
            "  {FRAMES} frames per scenario, first {WARMUP} discarded as warm-up."
        ));
        line("  CPU/frame is eframe's cpu_usage (egui + paint). Interval is wall time".into());
        line("  between frames and is VSYNC-BOUND: it cannot go below the display period,".into());
        line("  so only the CPU column says whether the UI can keep up.".into());
        line(String::new());
        line(format!(
            "  {:<34} {:>7} {:>7} {:>7} {:>7}   {:>8} {:>8}",
            "scenario", "cpu ms", "p50", "p95", "max", "iv p50", "iv max"
        ));
        for report in &bench.reports {
            line(format!(
                "  {:<34} {:>7.2} {:>7.2} {:>7.2} {:>7.2}   {:>8.2} {:>8.2}",
                report.scenario.title(),
                report.cpu_ms.mean,
                report.cpu_ms.p50,
                report.cpu_ms.p95,
                report.cpu_ms.max,
                report.interval_ms.p50,
                report.interval_ms.max,
            ));
        }
        line(String::new());
        // The stated bar, checked rather than assumed. 16.67 ms is one 60 Hz
        // frame; a p95 CPU cost above it means the UI thread alone cannot keep
        // up, whatever the display is doing.
        let budget = 1000.0 / 60.0;
        for report in &bench.reports {
            let verdict = if report.cpu_ms.p95 <= budget {
                "within a 60 fps budget"
            } else {
                "OVER a 60 fps budget"
            };
            line(format!(
                "  {:<34} p95 {:.2} ms of {budget:.2} ms -- {verdict}  ({} samples)",
                report.scenario.title(),
                report.cpu_ms.p95,
                report.samples
            ));
        }

        // Both, and the file is the one that matters: a release build sets
        // `windows_subsystem = "windows"`, so it has no console and `println!`
        // goes nowhere -- which is exactly the build whose numbers are worth
        // having.
        print!("{out}");
        let path = std::env::current_dir()
            .unwrap_or_default()
            .join(REPORT_FILE);
        match std::fs::write(&path, &out) {
            Ok(()) => println!("  written to {}", path.display()),
            Err(e) => println!("  could not write {}: {e}", path.display()),
        }
    }
}

/// Where the report is written, in the working directory.
const REPORT_FILE: &str = "vmiscope-bench.txt";
