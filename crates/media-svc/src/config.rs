//! Process configuration read from the environment. Storage, the bus and service auth read their own variables.

use anyhow::Context;

pub const DEFAULT_PORT: u16 = 8090;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    /// How many worker subprocesses may run at once.
    pub workers: usize,
    pub production: bool,
}

impl Config {
    /// Reads `PORT` (default 8090), `MEDIA_WORKERS` (default half the CPUs, at least 1) and `APP_ENV`.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let port = var("PORT").map(|v| v.parse().context("PORT must be a number")).unwrap_or(Ok(DEFAULT_PORT))?;
        let workers = match var("MEDIA_WORKERS") {
            Some(v) => match v.parse::<usize>() {
                Ok(n) if n > 0 => n,
                _ => anyhow::bail!("MEDIA_WORKERS must be a positive number"),
            },
            None => (cpus() / 2).max(1),
        };
        Ok(Self { port, workers, production: var("APP_ENV").is_some_and(|v| v == "production") })
    }
}

pub fn cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Threads each worker's rayon pool gets, so `workers` jobs at once share the CPUs.
pub fn rayon_threads(workers: usize) -> usize {
    (cpus() / workers.max(1)).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let config = Config::from_vars(|_| None).unwrap();
        assert_eq!(config.port, 8090);
        assert_eq!(config.workers, (cpus() / 2).max(1));
        assert!(!config.production);

        let config = Config::from_vars(|name| match name {
            "PORT" => Some("9001".into()),
            "MEDIA_WORKERS" => Some("3".into()),
            "APP_ENV" => Some("production".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!((config.port, config.workers, config.production), (9001, 3, true));
    }

    #[test]
    fn zero_workers_is_refused() {
        assert!(Config::from_vars(|name| (name == "MEDIA_WORKERS").then(|| "0".into())).is_err());
    }

    #[test]
    fn rayon_threads_never_reach_zero() {
        assert_eq!(rayon_threads(usize::MAX), 1);
        assert_eq!(rayon_threads(1), cpus());
    }
}
