//! The worker subprocess (`media-svc worker`) and the code that runs it.
//!
//! A native decoder that crashes or runs away takes only its worker down. The parent starts each worker in its own
//! process group, so a timeout kills the worker and the `cjxl`/`djxl` tools it runs together.
//!
//! **Input** (stdin): the [`ProcessJob`] as one line of JSON, then the upload's bytes until end of file.
//!
//! **Output** (stdout): a big-endian `u32` length, a JSON [`Header`] of that length, then the archive's bytes and
//! each rendition's bytes, back to back, at the lengths the header gives. Nothing else is written to stdout; the
//! worker logs to stderr.
//!
//! **Exit status:** 0 when a header was written (a finished job, or a file that cannot be read).
//! [`EXIT_OUT_OF_ADDRESS_SPACE`] when the worker found an allocation refused by its cap; a retry would meet the same
//! cap, so the job fails at once. Anything else (an encoder failure, a panic, a signal) is a crash, which the parent
//! may retry.

use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::process::{ExitCode, ExitStatus, Stdio};
use std::time::Duration;

use imaging::{ImagingError, SourceType};
use media_types::{Format, ProcessJob, Size};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Each worker's `RLIMIT_AS`. This caps virtual address space, not memory in use: rayon's threads, glibc's
/// per-thread arenas and the codecs reserve far more than they touch (a 12 MP job on 8 threads peaks near 1.8 GiB of
/// address space with about 1.3 GiB resident). It is a backstop against runaway allocation; decompression bombs are
/// stopped earlier by the 100 MP header check. cjxl and djxl inherit it, each in its own address space.
pub const WORKER_ADDRESS_SPACE: u64 = 3 << 30;
/// The worker's exit code when an allocation was refused by [`WORKER_ADDRESS_SPACE`].
pub const EXIT_OUT_OF_ADDRESS_SPACE: u8 = 3;
/// How long one attempt may run.
pub const TIMEOUT: Duration = Duration::from_secs(60);
const MAX_JOB_LINE: usize = 4096;
const MAX_HEADER: u32 = 1 << 20;
/// More output than any 25 MiB upload produces; a worker that writes past it is treated as broken.
const MAX_OUTPUT: u64 = 256 << 20;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum Header {
    Done { width: u32, height: u32, archive_ext: String, archive_len: u64, renditions: Vec<PieceHeader> },
    Unreadable { detail: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct PieceHeader {
    size: Size,
    format: Format,
    width: u32,
    height: u32,
    len: u64,
}

/// What a finished job produced.
#[derive(Debug)]
pub struct Output {
    pub width: u32,
    pub height: u32,
    pub archive: Vec<u8>,
    pub archive_ext: String,
    pub renditions: Vec<Piece>,
}

#[derive(Debug)]
pub struct Piece {
    pub size: Size,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum Outcome {
    Done(Output),
    /// The file is corrupt, of an unsupported type or too large; retrying cannot help.
    Unreadable(String),
    /// The job needs more address space than a worker has; retrying cannot help.
    OutOfAddressSpace,
}

/// An attempt that did not finish; another may.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("the worker could not run: {0}")]
    Io(#[from] io::Error),
    #[error("the worker ran out of time")]
    TimedOut,
    #[error("the worker exited with {0}")]
    Exited(ExitStatus),
    #[error("the worker's output was malformed: {0}")]
    Malformed(&'static str),
}

// ---------------------------------------------------------------------------------------------------------------
// The parent side
// ---------------------------------------------------------------------------------------------------------------

/// Runs one attempt of `job` on `bytes` in `<exe> worker`, with `rayon_threads` threads, killing the worker's whole
/// process group if it runs past `timeout`.
pub async fn run(
    exe: &Path,
    job: &ProcessJob,
    bytes: &[u8],
    rayon_threads: usize,
    timeout: Duration,
) -> Result<Outcome, WorkerError> {
    let mut child = tokio::process::Command::new(exe)
        .arg("worker")
        .env("RAYON_NUM_THREADS", rayon_threads.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()?;
    let group = child.id().and_then(|id| i32::try_from(id).ok()).and_then(rustix::process::Pid::from_raw);
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut line = serde_json::to_vec(job).expect("a job serialises");
    line.push(b'\n');

    let attempt = async {
        let feed = async move {
            // A worker that exits early closes the pipe; its exit status tells why, so a failed write is ignored.
            if stdin.write_all(&line).await.is_ok() {
                let _ = stdin.write_all(bytes).await;
            }
        };
        let collect = async {
            let mut out = Vec::new();
            (&mut stdout).take(MAX_OUTPUT + 1).read_to_end(&mut out).await.map(|_| out)
        };
        let ((), out, status) = tokio::join!(feed, collect, child.wait());
        (out, status)
    };
    let outcome = match tokio::time::timeout(timeout, attempt).await {
        Err(_) => Err(WorkerError::TimedOut),
        Ok((out, status)) => {
            let status = status?;
            if status.success() {
                parse(&out?)
            } else if status.code() == Some(i32::from(EXIT_OUT_OF_ADDRESS_SPACE)) {
                Ok(Outcome::OutOfAddressSpace)
            } else {
                Err(WorkerError::Exited(status))
            }
        }
    };
    if outcome.is_err() {
        // The tools a dead or stuck worker started share its group; none may outlive the attempt.
        if let Some(group) = group {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _ = child.wait().await;
    }
    outcome
}

fn parse(out: &[u8]) -> Result<Outcome, WorkerError> {
    if u64::try_from(out.len()).map_or(true, |len| len > MAX_OUTPUT) {
        return Err(WorkerError::Malformed("too much output"));
    }
    let (len, rest) = out.split_first_chunk::<4>().ok_or(WorkerError::Malformed("no header"))?;
    let len = u32::from_be_bytes(*len);
    if len > MAX_HEADER {
        return Err(WorkerError::Malformed("header too long"));
    }
    let (header, mut body) = rest.split_at_checked(len as usize).ok_or(WorkerError::Malformed("header cut short"))?;
    let header: Header = serde_json::from_slice(header).map_err(|_| WorkerError::Malformed("header is not JSON"))?;
    let mut take = |len: u64| -> Result<Vec<u8>, WorkerError> {
        let len = usize::try_from(len).map_err(|_| WorkerError::Malformed("length overflows"))?;
        let (piece, rest) = body.split_at_checked(len).ok_or(WorkerError::Malformed("body cut short"))?;
        body = rest;
        Ok(piece.to_vec())
    };
    let outcome = match header {
        Header::Unreadable { detail } => Outcome::Unreadable(detail),
        Header::Done { width, height, archive_ext, archive_len, renditions } => {
            let archive = take(archive_len)?;
            let renditions = renditions
                .into_iter()
                .map(|p| {
                    Ok(Piece { size: p.size, format: p.format, width: p.width, height: p.height, bytes: take(p.len)? })
                })
                .collect::<Result<_, WorkerError>>()?;
            Outcome::Done(Output { width, height, archive, archive_ext, renditions })
        }
    };
    if !body.is_empty() {
        return Err(WorkerError::Malformed("trailing bytes"));
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------------------------------------------
// The worker itself
// ---------------------------------------------------------------------------------------------------------------

/// `media-svc worker`: caps its own address space, reads one job, processes it and writes the result.
pub fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(io::stderr)
        .init();
    if let Err(error) = limit_memory() {
        tracing::error!(%error, "could not cap the worker's memory");
        return ExitCode::FAILURE;
    }
    #[cfg(debug_assertions)]
    if let Some(exit) = test_hooks::on_start() {
        return exit;
    }

    let mut input = Vec::new();
    if let Err(error) = io::stdin().lock().read_to_end(&mut input) {
        tracing::error!(%error, "reading the job failed");
        return ExitCode::FAILURE;
    }
    let Some(newline) = input.iter().take(MAX_JOB_LINE).position(|b| *b == b'\n') else {
        tracing::error!("the job line is missing or too long");
        return ExitCode::FAILURE;
    };
    let job: ProcessJob = match serde_json::from_slice(&input[..newline]) {
        Ok(job) => job,
        Err(error) => {
            tracing::error!(%error, "the job is not valid JSON");
            return ExitCode::FAILURE;
        }
    };
    let bytes = &input[newline + 1..];

    let result = match SourceType::from_mime(&job.mime) {
        Some(ty) => imaging::process(bytes, ty, job.purpose),
        None => Err(ImagingError::Unsupported),
    };
    let written = match result {
        Ok(processed) => {
            let header = Header::Done {
                width: processed.width,
                height: processed.height,
                archive_ext: processed.archive_ext.to_owned(),
                archive_len: processed.archive.len() as u64,
                renditions: processed
                    .renditions
                    .iter()
                    .map(|r| PieceHeader {
                        size: r.size,
                        format: r.format,
                        width: r.width,
                        height: r.height,
                        len: r.bytes.len() as u64,
                    })
                    .collect(),
            };
            let pieces = std::iter::once(processed.archive.as_slice())
                .chain(processed.renditions.iter().map(|r| r.bytes.as_slice()));
            write(&header, pieces)
        }
        Err(error @ (ImagingError::Corrupt(_) | ImagingError::Unsupported | ImagingError::TooLarge { .. })) => {
            tracing::info!(asset = %job.asset_id, %error, "the file cannot be read");
            write(&Header::Unreadable { detail: error.to_string() }, std::iter::empty())
        }
        Err(error @ ImagingError::Encode(_)) => {
            tracing::error!(asset = %job.asset_id, %error, "processing failed");
            return ExitCode::FAILURE;
        }
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "writing the result failed");
            ExitCode::FAILURE
        }
    }
}

/// Lowers the address-space limit to [`WORKER_ADDRESS_SPACE`], or keeps a lower limit already in place.
fn limit_memory() -> io::Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let cap = getrlimit(Resource::As).maximum.map_or(WORKER_ADDRESS_SPACE, |hard| hard.min(WORKER_ADDRESS_SPACE));
    setrlimit(Resource::As, Rlimit { current: Some(cap), maximum: Some(cap) })?;
    Ok(())
}

fn write<'a>(header: &Header, pieces: impl Iterator<Item = &'a [u8]>) -> io::Result<()> {
    let header = serde_json::to_vec(header).map_err(io::Error::other)?;
    let len = u32::try_from(header.len()).map_err(io::Error::other)?;
    let mut out = BufWriter::new(io::stdout().lock());
    out.write_all(&len.to_be_bytes())?;
    out.write_all(&header)?;
    for piece in pieces {
        out.write_all(piece)?;
    }
    out.flush()
}

/// Test hooks, compiled into debug builds only:
/// - `MEDIA_TEST_WORKER_CRASH=1`: abort at start;
/// - `MEDIA_TEST_WORKER_SPAWNS=<file>`: append a line per start;
/// - `MEDIA_TEST_WORKER_HANG=<file>`: start `sleep`, append "<worker pid> <sleep pid>", then never finish;
/// - `MEDIA_TEST_WORKER_ALLOC=<bytes>`: reserve that much first; refused, exit [`EXIT_OUT_OF_ADDRESS_SPACE`].
///
/// The parent honours `MEDIA_TEST_WORKER_TIMEOUT_MS` (see [`timeout`]).
#[cfg(debug_assertions)]
mod test_hooks {
    use std::fs::OpenOptions;
    use std::io::Write;

    fn append(path: &str, line: &str) {
        let mut file = OpenOptions::new().create(true).append(true).open(path).expect("open the hook file");
        writeln!(file, "{line}").expect("write the hook file");
    }

    /// `Some` when the worker should exit now with that code.
    pub(super) fn on_start() -> Option<std::process::ExitCode> {
        if let Ok(path) = std::env::var("MEDIA_TEST_WORKER_SPAWNS") {
            append(&path, "spawn");
        }
        if std::env::var("MEDIA_TEST_WORKER_CRASH").is_ok_and(|v| v == "1") {
            std::process::abort();
        }
        if let Ok(path) = std::env::var("MEDIA_TEST_WORKER_HANG") {
            // Detached from the worker's pipes, so a sleeper that outlives a broken test holds none of them open.
            let mut sleeper = std::process::Command::new("sleep")
                .arg("3600")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("start sleep");
            append(&path, &format!("{} {}", std::process::id(), sleeper.id()));
            let _ = sleeper.wait();
            loop {
                std::thread::park();
            }
        }
        if let Some(bytes) = std::env::var("MEDIA_TEST_WORKER_ALLOC").ok().and_then(|v| v.parse().ok()) {
            let mut block = Vec::<u8>::new();
            if block.try_reserve_exact(bytes).is_err() {
                tracing::error!(bytes, "an allocation was refused by the address-space cap");
                return Some(std::process::ExitCode::from(super::EXIT_OUT_OF_ADDRESS_SPACE));
            }
        }
        None
    }
}

/// How long an attempt may run: [`TIMEOUT`], or `MEDIA_TEST_WORKER_TIMEOUT_MS` in debug builds.
pub fn timeout() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("MEDIA_TEST_WORKER_TIMEOUT_MS").ok().and_then(|v| v.parse().ok()) {
        return Duration::from_millis(ms);
    }
    TIMEOUT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(header: &Header, body: &[u8]) -> Vec<u8> {
        let header = serde_json::to_vec(header).unwrap();
        let mut out = (header.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&header);
        out.extend_from_slice(body);
        out
    }

    fn done(archive_len: u64, rendition_len: u64) -> Header {
        Header::Done {
            width: 10,
            height: 5,
            archive_ext: "jxl".into(),
            archive_len,
            renditions: vec![PieceHeader {
                size: Size::Thumb,
                format: Format::Avif,
                width: 5,
                height: 5,
                len: rendition_len,
            }],
        }
    }

    #[test]
    fn output_splits_into_its_pieces() {
        let Outcome::Done(output) = parse(&frame(&done(3, 2), b"abcde")).unwrap() else { panic!("not done") };
        assert_eq!(output.archive, b"abc");
        assert_eq!(output.renditions[0].bytes, b"de");
        assert_eq!((output.width, output.height, output.archive_ext.as_str()), (10, 5, "jxl"));
    }

    #[test]
    fn short_or_long_output_is_malformed() {
        assert!(matches!(parse(&frame(&done(3, 2), b"abcd")), Err(WorkerError::Malformed(_))));
        assert!(matches!(parse(&frame(&done(3, 2), b"abcdef")), Err(WorkerError::Malformed(_))));
        assert!(matches!(parse(b""), Err(WorkerError::Malformed(_))));
        assert!(matches!(parse(&[0, 0, 0, 9, b'{']), Err(WorkerError::Malformed(_))));
    }

    #[test]
    fn unreadable_carries_its_detail() {
        let outcome = parse(&frame(&Header::Unreadable { detail: "corrupt".into() }, b"")).unwrap();
        assert!(matches!(outcome, Outcome::Unreadable(detail) if detail == "corrupt"));
    }
}
