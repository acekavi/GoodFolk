//! Process configuration read from the environment. Storage, the bus and service auth read their own variables.

use anyhow::Context;

pub const DEFAULT_PORT: u16 = 8090;

/// One 12 MP job needs about 8 threads to be ready within the 5 s gate, so CPUs go to a few wide workers rather
/// than many narrow ones.
pub const THREADS_PER_JOB: usize = 8;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    /// How many worker subprocesses may run at once.
    pub workers: usize,
    /// Threads each worker gets, for its rayon pool and for cjxl/djxl.
    pub worker_threads: usize,
    pub production: bool,
}

impl Config {
    /// Reads:
    /// - `PORT` (default 8090);
    /// - `MEDIA_WORKERS`: workers at once (default `max(1, cpus / 8)`, so 2 on 16 CPUs);
    /// - `MEDIA_WORKER_THREADS`: threads per worker (default `max(1, cpus / MEDIA_WORKERS)`, so 8 on 16 CPUs);
    /// - `APP_ENV`.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let port = var("PORT").map(|v| v.parse().context("PORT must be a number")).unwrap_or(Ok(DEFAULT_PORT))?;
        let workers = positive(&var, "MEDIA_WORKERS")?.unwrap_or_else(default_workers);
        let worker_threads = positive(&var, "MEDIA_WORKER_THREADS")?.unwrap_or_else(|| default_threads(workers));
        Ok(Self { port, workers, worker_threads, production: var("APP_ENV").is_some_and(|v| v == "production") })
    }
}

fn positive(var: &impl Fn(&str) -> Option<String>, name: &str) -> anyhow::Result<Option<usize>> {
    match var(name) {
        None => Ok(None),
        Some(v) => match v.parse::<usize>() {
            Ok(n) if n > 0 => Ok(Some(n)),
            _ => anyhow::bail!("{name} must be a positive number"),
        },
    }
}

pub fn cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

fn default_workers() -> usize {
    (cpus() / THREADS_PER_JOB).max(1)
}

/// `workers` jobs at once share the CPUs.
fn default_threads(workers: usize) -> usize {
    (cpus() / workers.max(1)).max(1)
}

/// Threads per worker for a service running `workers` at once: `MEDIA_WORKER_THREADS` when it is a positive
/// number (`serve` refuses to start on any other value), else the CPUs shared between the workers.
pub fn worker_threads(workers: usize) -> usize {
    positive(&|name| std::env::var(name).ok(), "MEDIA_WORKER_THREADS")
        .ok()
        .flatten()
        .unwrap_or_else(|| default_threads(workers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let config = Config::from_vars(|_| None).unwrap();
        assert_eq!(config.port, 8090);
        assert_eq!(config.workers, (cpus() / 8).max(1));
        assert_eq!(config.worker_threads, (cpus() / config.workers).max(1));
        assert!(!config.production);

        let config = Config::from_vars(|name| match name {
            "PORT" => Some("9001".into()),
            "MEDIA_WORKERS" => Some("3".into()),
            "MEDIA_WORKER_THREADS" => Some("5".into()),
            "APP_ENV" => Some("production".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!((config.port, config.workers, config.worker_threads, config.production), (9001, 3, 5, true));
    }

    #[test]
    fn zero_or_junk_is_refused() {
        for name in ["MEDIA_WORKERS", "MEDIA_WORKER_THREADS"] {
            assert!(Config::from_vars(|n| (n == name).then(|| "0".into())).is_err());
            assert!(Config::from_vars(|n| (n == name).then(|| "many".into())).is_err());
        }
    }

    #[test]
    fn threads_never_reach_zero() {
        assert_eq!(default_threads(usize::MAX), 1);
        assert_eq!(default_threads(1), cpus());
    }
}
