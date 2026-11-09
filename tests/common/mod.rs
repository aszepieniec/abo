//! What more than one test file needs.

include!("web_exe.rs");

/// abo-web, beside the abo these tests run.
pub fn web_exe() -> std::path::PathBuf {
    web_exe_in(std::path::Path::new(env!("CARGO_BIN_EXE_abo")).parent().unwrap())
}
