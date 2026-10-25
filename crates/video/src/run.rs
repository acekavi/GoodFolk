//! Runs one ffmpeg or ffprobe child under supervision: its own process group, a deadline, and capped output.

use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};

use crate::VideoError;

const MAX_STDERR: u64 = 64 * 1024;
const POLL: Duration = Duration::from_millis(10);

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

/// A child whose whole process group is killed when this is dropped, unless it has already been reaped (its pid may
/// then name someone else). A caller that unwinds or gives up mid-run leaves nothing running.
struct Group {
    child: Child,
    group: Option<Pid>,
}

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(group) = self.group {
            let _ = kill_process_group(group, Signal::KILL);
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Runs `command` in its own process group with no stdin, keeping the first `stdout_cap` bytes of its stdout. A child
/// still running at `deadline` is killed, with everything it started.
pub(crate) fn run(command: &mut Command, deadline: Instant, stdout_cap: u64) -> Result<Ran, VideoError> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| VideoError::Tool(e.to_string()))?;
    let group = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
    let mut child = Group { child, group };
    let stdout = drain(child.child.stdout.take(), stdout_cap);
    let stderr = drain(child.child.stderr.take(), MAX_STDERR);

    let status = loop {
        match child.child.try_wait() {
            Ok(Some(status)) => {
                child.group = None;
                break Ok(status);
            }
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
