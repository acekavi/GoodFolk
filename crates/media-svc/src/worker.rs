//! The worker subprocess (`media-svc worker`) and the code that runs it.
//!
//! A native decoder that crashes or runs away takes only its worker down. The parent starts each worker in its own
//! process group, so a timeout kills the worker and the `cjxl`/`djxl`/`ffmpeg`/`ffprobe` tools it runs together.
//!
//! **Input** (stdin), an image job: the [`ProcessJob`] as one line of JSON, then the upload's bytes until end of file.
//! A video job is the one line `{"job":<ProcessJob>,"input":"<path>","out_dir":"<path>"}` and nothing after it: the
//! upload is already in the file at `input`, and the worker writes what it makes into the directory `out_dir`.
//!
//! **Output** (stdout): a big-endian `u32` length, a JSON [`Header`] of that length, then, for an image, the archive's
//! bytes and each rendition's bytes, back to back, at the lengths the header gives. A video's header names its files
//! in `out_dir` instead, and nothing follows it. Nothing else is written to stdout; the worker logs to stderr.
//!
//! **Exit status:** 0 when a header was written (a finished job, or a file that cannot be read).
//! [`EXIT_OUT_OF_ADDRESS_SPACE`] when the worker found an allocation refused by its cap; a retry would meet the same
//! cap, so the job fails at once. Anything else (an encoder failure, a panic, a signal) is a crash, which the parent
//! may retry.

use std::io::{self, BufRead, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, ExitStatus, Stdio};
use std::time::Duration;

use imaging::{ImagingError, SourceType};
use media_types::{Format, ProcessJob, Size};
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, waitid};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use video::VideoError;

/// Each worker's `RLIMIT_AS`. This caps virtual address space, not memory in use: rayon's threads, glibc's
/// per-thread arenas and the codecs reserve far more than they touch (a 12 MP job on 8 threads peaks near 1.8 GiB of
/// address space with about 1.3 GiB resident). It is a backstop against runaway allocation; decompression bombs are
/// stopped earlier by the 100 MP header check. cjxl and djxl inherit it, each in its own address space.
pub const WORKER_ADDRESS_SPACE: u64 = 3 << 30;
/// A video worker's `RLIMIT_AS`, which each ffmpeg it runs inherits: two encodes at once, each with SVT-AV1's and
/// the decoder's threads, reserve far more than the 3 GiB an image job needs.
pub const VIDEO_WORKER_ADDRESS_SPACE: u64 = 8 << 30;
/// The worker's exit code when an allocation was refused by its address-space cap.
pub const EXIT_OUT_OF_ADDRESS_SPACE: u8 = 3;
/// How long one attempt may run.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// How long one attempt of a video job may run.
pub const VIDEO_TIMEOUT: Duration = Duration::from_secs(600);
/// What a video worker's environment adds, for the ffmpeg it runs. glibc gives each allocating thread an arena with
/// 64 MiB of address space reserved, up to 8 arenas per CPU. ffmpeg's hundred-odd threads then reserve past
/// [`VIDEO_WORKER_ADDRESS_SPACE`] (8.5 GB for one 1080p encode on 16 CPUs), and a refused reservation hangs SVT-AV1
/// rather than failing it. With 4 arenas the encode peaks near 4.4 GB, at the same speed.
const VIDEO_ENV: [(&str, &str); 1] = [("MALLOC_ARENA_MAX", "4")];
/// How long the worker's own `ffprobe` of a video may take; Finalize already probed it within the same time.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long extracting the poster frame may take.
const POSTER_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_JOB_LINE: usize = 4096;
const MAX_HEADER: u32 = 1 << 20;
/// More output than any 25 MiB upload produces; a worker that writes past it is treated as broken.
const MAX_OUTPUT: u64 = 256 << 20;

/// `Video` is a finished video: the displayed size and length of the source, and every file made, video renditions
/// first.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum Header {
    Done { width: u32, height: u32, archive_ext: String, archive_len: u64, renditions: Vec<PieceHeader> },
    Video { width: u32, height: u32, duration_ms: u32, files: Vec<VideoFile> },
    Unreadable { detail: String },
}

/// A video job's line.
#[derive(Debug, Serialize, Deserialize)]
struct VideoJob {
    job: ProcessJob,
    input: PathBuf,
    out_dir: PathBuf,
}

/// The job line, told apart by its shape.
#[derive(Deserialize)]
#[serde(untagged)]
enum JobLine {
    Video(VideoJob),
    Image(ProcessJob),
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

/// What a finished video job produced, in its `out_dir`.
#[derive(Debug)]
pub struct VideoOutput {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u32,
    pub files: Vec<VideoFile>,
}

/// One file a video job made: a video rendition or a poster image.
#[derive(Debug, Serialize, Deserialize)]
pub struct VideoFile {
    pub size: Size,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// A plain name inside the job's `out_dir`; [`run_video`] refuses anything that could point elsewhere.
    pub name: String,
}

#[derive(Debug)]
pub enum Outcome<T = Output> {
    Done(T),
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

/// Kills a worker's process group when dropped, so no worker or tool it started outlives an attempt, even one whose
/// future is dropped (a `/work` request cancelled mid-job).
struct GroupKill(Option<Pid>);

impl GroupKill {
    fn kill(&self) {
        if let Some(group) = self.0 {
            let _ = rustix::process::kill_process_group(group, Signal::KILL);
        }
    }

    /// Called once the worker is reaped: its pid may then be reused, so the group must not be signalled again.
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Waits until the worker has exited without reaping it, so its pid keeps naming the group until it is killed.
async fn exited(pid: Option<Pid>) {
    let Some(pid) = pid else { return };
    let _ = tokio::task::spawn_blocking(move || {
        while let Err(rustix::io::Errno::INTR) = waitid(WaitId::Pid(pid), WaitIdOptions::EXITED | WaitIdOptions::NOWAIT)
        {
        }
    })
    .await;
}

/// Runs one attempt of `job` on `bytes` in `<exe> worker`, with `rayon_threads` threads. When the worker exits, runs
/// past `timeout` or the attempt is dropped, its whole process group is killed before the worker is reaped.
pub async fn run(
    exe: &Path,
    job: &ProcessJob,
    bytes: &[u8],
    rayon_threads: usize,
    timeout: Duration,
) -> Result<Outcome, WorkerError> {
    let line = serde_json::to_vec(job).expect("a job serialises");
    match supervise(exe, line, bytes, &[], rayon_threads, timeout).await? {
        // Splitting up to 256 MiB of output is copying; keep it off the async workers.
        Exit::Output(out) => {
            tokio::task::spawn_blocking(move || parse(&out)).await.map_err(|e| WorkerError::Io(io::Error::other(e)))?
        }
        Exit::OutOfAddressSpace => Ok(Outcome::OutOfAddressSpace),
    }
}

/// Runs one attempt of the video `job` on the file `input` in `<exe> worker`, as [`run`] does, writing its files into
/// `out_dir`.
pub async fn run_video(
    exe: &Path,
    job: &ProcessJob,
    input: &Path,
    out_dir: &Path,
    rayon_threads: usize,
    timeout: Duration,
) -> Result<Outcome<VideoOutput>, WorkerError> {
    let line = VideoJob { job: job.clone(), input: input.to_owned(), out_dir: out_dir.to_owned() };
    // Only a path that is not UTF-8 fails to serialise.
    let line = serde_json::to_vec(&line).map_err(|e| WorkerError::Io(io::Error::other(e)))?;
    match supervise(exe, line, &[], &VIDEO_ENV, rayon_threads, timeout).await? {
        Exit::Output(out) => parse_video(&out),
        Exit::OutOfAddressSpace => Ok(Outcome::OutOfAddressSpace),
    }
}

/// How a worker that ran to the end exited.
enum Exit {
    /// Successfully, with what it wrote to stdout.
    Output(Vec<u8>),
    OutOfAddressSpace,
}

/// Runs `<exe> worker` once with `env` added to its environment, feeding it the job `line` and then `bytes`.
async fn supervise(
    exe: &Path,
    mut line: Vec<u8>,
    bytes: &[u8],
    env: &[(&str, &str)],
    rayon_threads: usize,
    timeout: Duration,
) -> Result<Exit, WorkerError> {
    let mut command = tokio::process::Command::new(exe);
    // The worker parses hostile files, so it gets none of the service's secrets: only what it needs to run.
    command.arg("worker").env_clear().envs(forwarded_env()).env("RAYON_NUM_THREADS", rayon_threads.to_string());
    command.envs(env.iter().copied());
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id().and_then(|id| i32::try_from(id).ok()).and_then(Pid::from_raw);
    let mut group = GroupKill(pid);
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = child.stdout.take().expect("stdout is piped");
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
        // A worker cannot exit before its output is in the pipe, so killing what is left of the group then loses
        // nothing, and a tool a crashed worker left behind cannot hold the pipe open.
        let killed = async {
            exited(pid).await;
            group.kill();
        };
        let ((), out, ()) = tokio::join!(feed, collect, killed);
        out
    };
    let finished = tokio::time::timeout(timeout, attempt).await;
    group.kill();
    let status = child.wait().await;
    group.disarm();

    let out = finished.map_err(|_| WorkerError::TimedOut)?;
    let status = status?;
    if status.success() {
        Ok(Exit::Output(out?))
    } else if status.code() == Some(i32::from(EXIT_OUT_OF_ADDRESS_SPACE)) {
        Ok(Exit::OutOfAddressSpace)
    } else {
        Err(WorkerError::Exited(status))
    }
}

/// The variables a worker inherits: where the JPEG XL tools and ffmpeg are (`JXL_BIN_DIR` and `FFMPEG_BIN_DIR`, else
/// under `HOME`), `PATH` and `RUST_LOG`, plus the test hooks' `MEDIA_TEST_WORKER_*` in debug builds.
/// `RAYON_NUM_THREADS` is set per worker.
fn forwarded_env() -> impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)> {
    std::env::vars_os().filter(|(name, _)| {
        let Some(name) = name.to_str() else { return false };
        #[cfg(debug_assertions)]
        if name.starts_with("MEDIA_TEST_WORKER_") {
            return true;
        }
        matches!(name, "JXL_BIN_DIR" | "FFMPEG_BIN_DIR" | "HOME" | "PATH" | "RUST_LOG")
    })
}

/// The header at the start of `out`, and the bytes after it.
fn header(out: &[u8]) -> Result<(Header, &[u8]), WorkerError> {
    if u64::try_from(out.len()).map_or(true, |len| len > MAX_OUTPUT) {
        return Err(WorkerError::Malformed("too much output"));
    }
    let (len, rest) = out.split_first_chunk::<4>().ok_or(WorkerError::Malformed("no header"))?;
    let len = u32::from_be_bytes(*len);
    if len > MAX_HEADER {
        return Err(WorkerError::Malformed("header too long"));
    }
    let (header, body) = rest.split_at_checked(len as usize).ok_or(WorkerError::Malformed("header cut short"))?;
    let header: Header = serde_json::from_slice(header).map_err(|_| WorkerError::Malformed("header is not JSON"))?;
    Ok((header, body))
}

fn parse(out: &[u8]) -> Result<Outcome, WorkerError> {
    let (header, mut body) = header(out)?;
    let mut take = |len: u64| -> Result<Vec<u8>, WorkerError> {
        let len = usize::try_from(len).map_err(|_| WorkerError::Malformed("length overflows"))?;
        let (piece, rest) = body.split_at_checked(len).ok_or(WorkerError::Malformed("body cut short"))?;
        body = rest;
        Ok(piece.to_vec())
    };
    let outcome = match header {
        Header::Unreadable { detail } => Outcome::Unreadable(detail),
        Header::Video { .. } => return Err(WorkerError::Malformed("a video result for an image job")),
        Header::Done { archive_ext, .. } if !matches!(archive_ext.as_str(), "jxl" | "jpg" | "pdf") => {
            return Err(WorkerError::Malformed("unknown archive type"));
        }
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

fn parse_video(out: &[u8]) -> Result<Outcome<VideoOutput>, WorkerError> {
    let (header, body) = header(out)?;
    if !body.is_empty() {
        return Err(WorkerError::Malformed("trailing bytes"));
    }
    match header {
        Header::Unreadable { detail } => Ok(Outcome::Unreadable(detail)),
        Header::Done { .. } => Err(WorkerError::Malformed("an image result for a video job")),
        Header::Video { files, .. }
            if files.iter().any(|f| f.name.is_empty() || f.name.contains('/') || f.name.starts_with('.')) =>
        {
            Err(WorkerError::Malformed("a file name outside the output directory"))
        }
        Header::Video { width, height, duration_ms, files } => {
            Ok(Outcome::Done(VideoOutput { width, height, duration_ms, files }))
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The worker itself
// ---------------------------------------------------------------------------------------------------------------

/// `media-svc worker`: reads one job, caps its own address space for that kind of job, processes it and writes the
/// result.
pub fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(io::stderr)
        .init();

    let mut stdin = io::stdin().lock();
    let mut line = Vec::new();
    if let Err(error) = (&mut stdin).take(MAX_JOB_LINE as u64).read_until(b'\n', &mut line) {
        tracing::error!(%error, "reading the job failed");
        return ExitCode::FAILURE;
    }
    if line.pop() != Some(b'\n') {
        tracing::error!("the job line is missing or too long");
        return ExitCode::FAILURE;
    }
    let job: JobLine = match serde_json::from_slice(&line) {
        Ok(job) => job,
        Err(error) => {
            tracing::error!(%error, "the job is not valid JSON");
            return ExitCode::FAILURE;
        }
    };
    let cap = match job {
        JobLine::Video(_) => VIDEO_WORKER_ADDRESS_SPACE,
        JobLine::Image(_) => WORKER_ADDRESS_SPACE,
    };
    if let Err(error) = limit_memory(cap) {
        tracing::error!(%error, "could not cap the worker's memory");
        return ExitCode::FAILURE;
    }
    #[cfg(debug_assertions)]
    if let Some(exit) = test_hooks::on_start() {
        return exit;
    }

    let mut bytes = Vec::new();
    if let Err(error) = stdin.read_to_end(&mut bytes) {
        tracing::error!(%error, "reading the job failed");
        return ExitCode::FAILURE;
    }
    match job {
        JobLine::Image(job) => image(&job, &bytes),
        JobLine::Video(_) if !bytes.is_empty() => {
            tracing::error!("bytes follow a video job's line");
            ExitCode::FAILURE
        }
        JobLine::Video(job) => video(&job),
    }
}

fn image(job: &ProcessJob, bytes: &[u8]) -> ExitCode {
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
    finish(written)
}

/// Why a video job did not finish.
enum Failure {
    /// The file cannot be made into renditions; retrying cannot help.
    Unreadable(String),
    /// A tool failed or ran out of time; another attempt may do better.
    Crashed(String),
}

impl From<VideoError> for Failure {
    fn from(error: VideoError) -> Self {
        match error {
            VideoError::Unreadable(_) => Self::Unreadable(error.to_string()),
            VideoError::Tool(_) | VideoError::TimedOut => Self::Crashed(error.to_string()),
        }
    }
}

impl From<ImagingError> for Failure {
    fn from(error: ImagingError) -> Self {
        match error {
            ImagingError::Corrupt(_) | ImagingError::Unsupported | ImagingError::TooLarge { .. } => {
                Self::Unreadable(error.to_string())
            }
            ImagingError::Encode(_) => Self::Crashed(error.to_string()),
        }
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Crashed(error.to_string())
    }
}

fn video(job: &VideoJob) -> ExitCode {
    let written = match transcode(job) {
        Ok(header) => write(&header, std::iter::empty()),
        Err(Failure::Unreadable(detail)) => {
            tracing::info!(asset = %job.job.asset_id, detail, "the video cannot be read");
            write(&Header::Unreadable { detail }, std::iter::empty())
        }
        Err(Failure::Crashed(detail)) => {
            tracing::error!(asset = %job.job.asset_id, detail, "processing failed");
            return ExitCode::FAILURE;
        }
    };
    finish(written)
}

/// Probes the video again (Finalize accepted it, but the worker trusts only what it sees), then encodes the renditions
/// while the poster is made beside them.
fn transcode(job: &VideoJob) -> Result<Header, Failure> {
    let probe = video::probe(&job.input, PROBE_TIMEOUT)?;
    video::accept(&probe).map_err(|refusal| Failure::Unreadable(refusal.message().to_owned()))?;
    let (encoded, posters) = std::thread::scope(|scope| {
        let posters = scope.spawn(|| poster(job, &probe));
        let encoded = video::transcode(&job.input, &probe, &job.out_dir, &video::Options::default());
        (encoded, posters.join())
    });
    let posters = posters.map_err(|_| Failure::Crashed("the poster thread panicked".to_owned()))??;
    let mut files = encoded?
        .into_iter()
        .map(|e| {
            let name = e.path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
            let name = name.ok_or_else(|| Failure::Crashed(format!("{} has no file name", e.path.display())))?;
            Ok(VideoFile { size: e.size, format: e.format, width: e.width, height: e.height, name })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    files.extend(posters);
    Ok(Header::Video { width: probe.width, height: probe.height, duration_ms: probe.duration_ms, files })
}

/// The poster frame through the Gallery's image ladder, each rendition written to `<size>.<format>` in `out_dir`.
fn poster(job: &VideoJob, probe: &video::Probe) -> Result<Vec<VideoFile>, Failure> {
    let png = video::poster_png(&job.input, probe, POSTER_TIMEOUT)?;
    imaging::poster_renditions(&png)?
        .into_iter()
        .map(|r| {
            let name = format!("{}.{}", r.size.as_str(), r.format.as_str());
            std::fs::write(job.out_dir.join(&name), &r.bytes)?;
            Ok(VideoFile { size: r.size, format: r.format, width: r.width, height: r.height, name })
        })
        .collect()
}

fn finish(written: io::Result<()>) -> ExitCode {
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "writing the result failed");
            ExitCode::FAILURE
        }
    }
}

/// Lowers the address-space limit to `cap`, or keeps a lower limit already in place.
fn limit_memory(cap: u64) -> io::Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let cap = getrlimit(Resource::As).maximum.map_or(cap, |hard| hard.min(cap));
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
/// - `MEDIA_TEST_WORKER_ALLOC=<bytes>`: reserve that much first; refused, exit [`EXIT_OUT_OF_ADDRESS_SPACE`];
/// - `MEDIA_TEST_WORKER_SECRETS=<file>`: append a line naming each service secret visible to the worker (empty when
///   none is).
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
        if let Ok(path) = std::env::var("MEDIA_TEST_WORKER_SECRETS") {
            let visible: Vec<&str> = ["S3_SECRET_KEY", "INTERNAL_SHARED_SECRET"]
                .into_iter()
                .filter(|name| std::env::var_os(name).is_some())
                .collect();
            append(&path, &visible.join(" "));
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
    fn an_unknown_archive_type_is_malformed() {
        let Header::Done { width, height, archive_len, renditions, .. } = done(3, 2) else { unreachable!() };
        let header = Header::Done { width, height, archive_ext: "../x".into(), archive_len, renditions };
        assert!(matches!(parse(&frame(&header, b"abcde")), Err(WorkerError::Malformed(_))));
    }

    #[test]
    fn every_archive_type_the_worker_writes_is_accepted() {
        for ext in ["jxl", "jpg", "pdf"] {
            let Header::Done { width, height, archive_len, renditions, .. } = done(3, 2) else { unreachable!() };
            let header = Header::Done { width, height, archive_ext: ext.into(), archive_len, renditions };
            let Outcome::Done(output) = parse(&frame(&header, b"abcde")).unwrap() else { panic!("not done") };
            assert_eq!(output.archive_ext, ext);
        }
    }

    #[test]
    fn unreadable_carries_its_detail() {
        let outcome = parse(&frame(&Header::Unreadable { detail: "corrupt".into() }, b"")).unwrap();
        assert!(matches!(outcome, Outcome::Unreadable(detail) if detail == "corrupt"));
        let outcome = parse_video(&frame(&Header::Unreadable { detail: "corrupt".into() }, b"")).unwrap();
        assert!(matches!(outcome, Outcome::Unreadable(detail) if detail == "corrupt"));
    }

    fn video(name: &str) -> Header {
        Header::Video {
            width: 1920,
            height: 1080,
            duration_ms: 10_000,
            files: vec![VideoFile {
                size: Size::V1080,
                format: Format::Av1,
                width: 1920,
                height: 1080,
                name: name.into(),
            }],
        }
    }

    #[test]
    fn a_video_result_names_its_files() {
        let Outcome::Done(output) = parse_video(&frame(&video("1080p.av1.mp4"), b"")).unwrap() else {
            panic!("not done")
        };
        assert_eq!((output.width, output.height, output.duration_ms), (1920, 1080, 10_000));
        assert_eq!(output.files[0].name, "1080p.av1.mp4");
    }

    #[test]
    fn a_video_file_outside_the_output_directory_is_malformed() {
        for name in ["../upload", "/etc/passwd", "a/b.mp4", ".hidden", ""] {
            assert!(matches!(parse_video(&frame(&video(name), b"")), Err(WorkerError::Malformed(_))), "{name:?}");
        }
    }

    #[test]
    fn a_result_of_the_other_kind_or_with_bytes_after_a_video_is_malformed() {
        assert!(matches!(parse(&frame(&video("1080p.av1.mp4"), b"")), Err(WorkerError::Malformed(_))));
        assert!(matches!(parse_video(&frame(&done(3, 2), b"abcde")), Err(WorkerError::Malformed(_))));
        assert!(matches!(parse_video(&frame(&video("1080p.av1.mp4"), b"x")), Err(WorkerError::Malformed(_))));
    }

    #[test]
    fn the_job_line_tells_a_video_from_an_image() {
        let job = ProcessJob {
            tenant_id: uuid::Uuid::now_v7(),
            asset_id: uuid::Uuid::now_v7(),
            purpose: media_types::Profile::GalleryVideo,
            mime: "video/mp4".into(),
        };
        let image = serde_json::to_vec(&job).unwrap();
        assert!(matches!(serde_json::from_slice(&image).unwrap(), JobLine::Image(_)));
        let line = VideoJob { job, input: "/tmp/a/source".into(), out_dir: "/tmp/a/out".into() };
        let line = serde_json::to_vec(&line).unwrap();
        assert!(
            matches!(serde_json::from_slice(&line).unwrap(), JobLine::Video(v) if v.out_dir == Path::new("/tmp/a/out"))
        );
    }
}
