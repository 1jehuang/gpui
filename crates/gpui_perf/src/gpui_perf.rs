//! Simulated application scenarios for measuring what a frame costs.
//!
//! Each [`Scenario`] builds a window's worth of UI shaped like a common
//! application screen — a long form, a large list, a data table, a settings
//! page — and then, frame after frame, changes it the way a user or live data
//! would: typing into a field, toggling a checkbox, scrolling, selecting a
//! row, a value ticking. The runner in `runner.rs` drives every scenario
//! headlessly, with real text shaping, once with retained views and once
//! without, and reports what each frame cost.

pub mod runner;
pub mod scenarios;

use gpui::{AnyView, App, Window};

/// One simulated workload.
///
/// `build` creates the root view once. `step` is then called before every
/// frame with the frame's number, and changes whatever this frame changes,
/// the way the application would: updating entities and notifying them, or
/// dispatching input events to the window. It must be deterministic, so two
/// runs draw the same frames.
pub trait Scenario {
    /// A short, unique, kebab-case name, e.g. `form-typing`.
    fn name(&self) -> &'static str;

    /// One sentence on what the scenario simulates.
    fn description(&self) -> &'static str;

    /// Builds the scenario's root view.
    fn build(&self, window: &mut Window, cx: &mut App) -> AnyView;

    /// Changes what frame `frame` changes. `root` is the view `build`
    /// returned.
    fn step(&self, root: &AnyView, frame: usize, window: &mut Window, cx: &mut App);
}

/// Every scenario, in the order they are reported.
pub fn all_scenarios() -> Vec<Box<dyn Scenario>> {
    let mut scenarios = Vec::new();
    scenarios.extend(scenarios::form::scenarios());
    scenarios.extend(scenarios::list::scenarios());
    scenarios.extend(scenarios::table::scenarios());
    scenarios.extend(scenarios::settings::scenarios());
    scenarios
}
