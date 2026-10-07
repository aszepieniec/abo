//! The command line: what clap reads, what each command does, and the
//! questions it asks on the terminal.

pub(crate) mod args;
mod prompt;
mod run;

pub(crate) use args::Cli;
#[cfg(test)]
pub(crate) use args::PASSED_HELP;
pub(crate) use run::{hints, run};
