//! Measures what simulated application screens cost to draw, frame by frame,
//! with and without retained views.
//!
//! ```text
//! cargo run -p gpui_perf --release -- --frames 200
//! ```
//!
//! Flags:
//!
//! - `--scenario <substring>`: run only scenarios whose name contains it;
//!   repeatable.
//! - `--frames N`: frames measured per run (default 200).
//! - `--warmup N`: frames drawn before measuring (default 30).
//! - `--retention on|off|both`: which modes to run (default both).
//! - `--json PATH`: also write every result as JSON.
//! - `--verify`: also run both modes in lockstep and check they paint the
//!   same quads every frame.
//! - `--list`: print the scenarios and exit.

use std::process::ExitCode;

use gpui_perf::runner::{self, Options, RetentionModes};

const USAGE: &str = "usage: gpui_perf [--scenario SUBSTRING]... [--frames N] [--warmup N] \
[--retention on|off|both] [--json PATH] [--verify] [--list]";

fn main() -> ExitCode {
    let mut options = Options::default();
    let mut json_path = None;
    let mut list = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next().unwrap_or_else(|| {
                eprintln!("{name} needs a value\n{USAGE}");
                std::process::exit(2);
            })
        };
        let number = |name: &str, value: String| {
            value.parse::<usize>().unwrap_or_else(|_| {
                eprintln!("{name} needs a number, got {value:?}");
                std::process::exit(2);
            })
        };
        match arg.as_str() {
            "--scenario" => options.filters.push(value("--scenario")),
            "--frames" => options.frames = number("--frames", value("--frames")),
            "--warmup" => options.warmup = number("--warmup", value("--warmup")),
            "--retention" => {
                options.retention = match value("--retention").as_str() {
                    "on" => RetentionModes::On,
                    "off" => RetentionModes::Off,
                    "both" => RetentionModes::Both,
                    other => {
                        eprintln!("--retention takes on, off or both, got {other:?}");
                        return ExitCode::from(2);
                    }
                }
            }
            "--json" => json_path = Some(value("--json")),
            "--verify" => options.verify = true,
            "--list" => list = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let selected = runner::selected_scenarios(&options);
    if list {
        for (_, name, description) in &selected {
            println!("{name:<32}{description}");
        }
        return ExitCode::SUCCESS;
    }
    if selected.is_empty() {
        eprintln!("no scenario matches");
        return ExitCode::FAILURE;
    }

    let reports = runner::run(&options);
    print!("{}", runner::format_reports(&reports));

    if let Some(path) = json_path {
        if let Err(error) = std::fs::write(&path, runner::to_json(&options, &reports)) {
            eprintln!("failed to write {path}: {error}");
            return ExitCode::FAILURE;
        }
        eprintln!("wrote {path}");
    }

    if reports
        .iter()
        .any(|report| report.verify.as_ref().is_some_and(|verify| !verify.passed))
    {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
