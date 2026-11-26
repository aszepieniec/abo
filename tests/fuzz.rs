//! The fuzzers of tests/fuzz/, run against the built abo: random sequences
//! of corporate commands checked against a model of the tree, and long mixed
//! histories, bookkeeping and corporate, undone, redone and replayed. Each
//! seed is fixed, so a run is the same each time; `ABO_FUZZ=n` runs n times
//! as many seeds, and `ABO_FUZZ_SEED=s` starts at seed s, for new ground.
//! They need python3; without it they say so and pass.

use std::path::PathBuf;
use std::process::Command;

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// `script` with `args` after abo and a directory of its own; fails with
/// what it printed if it found an issue.
fn fuzz(script: &str, args: &[String]) {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 is not there: {script} not run");
        return;
    }
    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("fuzz-{script}-{}", args.join("-")));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fuzz").join(script);
    let o = Command::new("python3").arg("-I").arg(path).arg(env!("CARGO_BIN_EXE_abo")).arg(&work).args(args).output().unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(o.status.success(), "{script} {args:?} found issues; its work is in {}:\n{said}", work.display());
    let _ = std::fs::remove_dir_all(&work);
}

/// G-C1, G-C2, G-O8 for the corporate commands, with failures injected: a
/// hook that refuses, git's index held, `.abo` not writable, a directory
/// not writable, a full disk.
#[test]
fn corporate_commands_hold_against_a_model() {
    let (from, n) = (knob("ABO_FUZZ_SEED", 0), 16 * knob("ABO_FUZZ", 1));
    fuzz("corporate.py", &[from.to_string(), (from + n).to_string()]);
}

/// Long histories, every kind of operation among them, the corporate tree
/// against a model after each step, and the log replayed into new books.
#[test]
fn long_histories_replay() {
    let from = knob("ABO_FUZZ_SEED", 1);
    for seed in from..from + knob("ABO_FUZZ", 1) {
        fuzz("history.py", &[seed.to_string(), "120".into()]);
    }
}
