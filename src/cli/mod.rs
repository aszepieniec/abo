//! The command line: what clap reads, what each command does, and the
//! questions it asks on the terminal.

pub(crate) mod args;
mod prompt;
mod run;

pub(crate) use args::Cli;
pub(crate) use run::run;
