//! Runs one ffmpeg or ffprobe child under supervision: a deadline, and capped output. Neither tool starts children of
//! its own, so the child stays in the caller's process group, where a supervisor's group kill (media-svc's worker
//! kill) reaches it too.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::VideoError;

const MAX_STDERR: u64 = 64 * 1024;
const POLL: Duration = Duration::from_millis(10);

/// The arguments that open `path` as an ffmpeg or ffprobe input: always read as a file, never as a protocol (`http:`,
/// `concat:`, ...), and nothing the file refers to (a playlist's segments, say) opened through any protocol but `file`.
pub(crate) fn input(path: &Path) -> [OsString; 4] {
    let mut file = OsString::from("file:");
    file.push(path);
    ["-protocol_whitelist".into(), "file".into(), "-i".into(), file]
}

/// What a finished child left behind.
pub(crate) struct Ran {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    /// Whether stdout ran past its cap; the excess was discarded.
    pub stdout_over: bool,
    pub stderr: Vec<u8>,
}

impl Ran {
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

/// A child that is killed (SIGKILL) and reaped when this is dropped, so a caller that times out, unwinds or gives up
/// mid-run leaves nothing running. Once the child has been reaped, `Child::kill` signals nothing.
struct Supervised(Child);

impl Drop for Supervised {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs `command` with no stdin, keeping the first `stdout_cap` bytes of its stdout. A child still running at
/// `deadline` is killed.
pub(crate) fn run(command: &mut Command, deadline: Instant, stdout_cap: u64) -> Result<Ran, VideoError> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| VideoError::Tool(e.to_string()))?;
    let mut child = Supervised(child);
    let stdout = drain(child.0.stdout.take(), stdout_cap);
    let stderr = drain(child.0.stderr.take(), MAX_STDERR);

    let status = loop {
        match child.0.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline => break Err(VideoError::TimedOut),
            Ok(None) => thread::sleep(POLL),
            Err(e) => break Err(VideoError::Tool(e.to_string())),
        }
    };
    // Killed (when still running) before the pipes are joined, so they reach their end.
    drop(child);
    let (stdout, stdout_over) = stdout.join().unwrap_or_default();
    let (stderr, _) = stderr.join().unwrap_or_default();
    Ok(Ran { status: status?, stdout, stdout_over, stderr })
}

/// Reads `pipe` to its end, keeping the first `cap` bytes and whether there were more. The rest is discarded as it
/// arrives, so the child never blocks on a full pipe.
fn drain(pipe: Option<impl Read + Send + 'static>, cap: u64) -> thread::JoinHandle<(Vec<u8>, bool)> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut over = false;
        if let Some(mut pipe) = pipe {
            let _ = (&mut pipe).take(cap).read_to_end(&mut buf);
            over = io::copy(&mut pipe, &mut io::sink()).is_ok_and(|rest| rest > 0);
        }
        (buf, over)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_input_is_a_file_that_opens_only_files() {
        let args = input(Path::new("/tmp/clip.mp4"));
        assert_eq!(args, ["-protocol_whitelist", "file", "-i", "file:/tmp/clip.mp4"].map(OsString::from));
    }
}
