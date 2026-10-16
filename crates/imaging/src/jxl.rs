//! Runs libjxl's official `cjxl` and `djxl` tools as subprocesses, bytes in and bytes out.

use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

const BIN_DIR_VAR: &str = "JXL_BIN_DIR";
const STDERR_LIMIT: usize = 300;

/// A tool failure: the tool is missing, it ran and said no (exited with an error status, which running it again
/// would repeat), or it could not run to the end (it would not start, or a signal killed it).
pub(crate) enum ToolError {
    Missing(String),
    Refused(String),
    Failed(String),
}

fn bin_dir() -> Result<PathBuf, ToolError> {
    if let Some(dir) = std::env::var_os(BIN_DIR_VAR) {
        return Ok(PathBuf::from(dir));
    }
    let home =
        std::env::var_os("HOME").ok_or_else(|| ToolError::Missing(format!("neither {BIN_DIR_VAR} nor HOME is set")))?;
    Ok(PathBuf::from(home).join(".cache/goodfolk-libjxl/bin"))
}

/// Runs `tool` with `args`, feeding `input` on stdin and returning stdout. The tool gets as many threads as rayon's
/// global pool (sized by `RAYON_NUM_THREADS`), so it keeps to the caller's CPU share; left alone it takes every core.
pub(crate) fn run(tool: &str, args: &[&str], input: &[u8]) -> Result<Vec<u8>, ToolError> {
    let path = bin_dir()?.join(tool);
    let mut child = Command::new(&path)
        .args(args)
        .arg(format!("--num_threads={}", rayon::current_num_threads()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ToolError::Missing(format!("{tool} not found at {}; run deploy/dev/libjxl.sh install", path.display()))
            } else {
                ToolError::Failed(format!("{tool} could not start: {e}"))
            }
        })?;

    let mut stdin = child.stdin.take().expect("stdin was piped");
    let output = std::thread::scope(|scope| {
        // Feed stdin from another thread so a tool that writes before it has read everything
        // cannot deadlock against us. A tool that exits early closes the pipe; its exit status
        // then tells the story, so the write error is ignored.
        scope.spawn(move || {
            let _ = stdin.write_all(input);
        });
        child.wait_with_output()
    })
    .map_err(|e| ToolError::Failed(format!("{tool} failed: {e}")))?;

    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr: String = stderr.trim().chars().take(STDERR_LIMIT).collect();
    let message = format!("{tool} exited with {}: {stderr}", output.status);
    Err(if output.status.code().is_some() { ToolError::Refused(message) } else { ToolError::Failed(message) })
}
