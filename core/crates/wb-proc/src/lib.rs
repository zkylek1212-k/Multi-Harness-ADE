//! Launching CLIs the way `src/main/cowork/runner.ts` does, plus process tree termination.
pub mod job;
pub mod launch;
pub mod runner;

pub use job::KillOnCloseJob;
pub use launch::{launch_plan, resolve_command, LaunchPlan, Resolved};
pub use runner::{run_process, CancelToken, RunResult, RunSpec};
