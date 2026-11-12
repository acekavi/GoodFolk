use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::sync::Semaphore;

/// Most password hashes (sign-ups, sign-ins and accepted invitations together) this process computes at once. Each
/// one holds Argon2id's 19 MiB and a blocking thread for tens of milliseconds, so without a bound a flood of sign-in
/// attempts with new emails (each passing the per-email throttle) could exhaust memory.
pub const MAX_CONCURRENT_HASHES: usize = 4;
/// How long a request waits for a free hashing slot before it is refused with [`Busy`].
pub const HASH_WAIT: Duration = Duration::from_secs(3);

static HASHING: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_HASHES)));

/// Every hashing slot stayed busy for [`HASH_WAIT`]: the request is refused and may be tried again shortly.
#[derive(Debug, thiserror::Error)]
#[error("the server is busy; try again in a moment")]
pub struct Busy;

/// Hashes with Argon2id at the crate's default (OWASP-recommended) parameters and a random salt.
/// CPU-heavy: call from `tokio::task::spawn_blocking`.
pub fn hash_password(password: &str) -> String {
    Argon2::default()
        .hash_password(password.as_bytes())
        .expect("argon2 hashing with default params cannot fail")
        .to_string()
}

/// CPU-heavy: call from `tokio::task::spawn_blocking`.
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

/// Runs `work` (a hash or a verification) on the blocking pool once one of the [`MAX_CONCURRENT_HASHES`] slots is
/// free, or answers [`Busy`] after [`HASH_WAIT`].
pub(crate) async fn bounded<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, Busy> {
    run_bounded(&HASHING, HASH_WAIT, work).await
}

/// The slot is held by the blocking task itself, so a request dropped at its deadline keeps its slot until the hash
/// it started has finished: abandoned hashes still count against the bound.
async fn run_bounded<T: Send + 'static>(
    slots: &Arc<Semaphore>,
    wait: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Busy> {
    let slot = tokio::time::timeout(wait, slots.clone().acquire_owned())
        .await
        .map_err(|_| Busy)?
        .expect("the hashing semaphore is never closed");
    let done = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        work()
    })
    .await
    .expect("a hashing task does not panic");
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::{Busy, run_bounded};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn work_waits_for_a_free_slot_and_is_refused_once_the_wait_runs_out() {
        let slots = Arc::new(Semaphore::new(1));
        let (release, released) = std::sync::mpsc::channel::<()>();
        let holder = {
            let slots = slots.clone();
            tokio::spawn(async move { run_bounded(&slots, Duration::from_secs(5), move || released.recv()).await })
        };
        while slots.available_permits() == 1 {
            tokio::task::yield_now().await;
        }

        let refused = run_bounded(&slots, Duration::from_millis(50), || 1).await;
        let waiting = {
            let slots = slots.clone();
            tokio::spawn(async move { run_bounded(&slots, Duration::from_secs(5), || 2).await })
        };
        release.send(()).unwrap();

        assert!(matches!(refused, Err(Busy)));
        assert!(holder.await.unwrap().is_ok());
        assert_eq!(waiting.await.unwrap().unwrap(), 2);
        assert_eq!(slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn a_dropped_request_keeps_its_slot_until_its_hash_finishes() {
        let slots = Arc::new(Semaphore::new(1));
        let (release, released) = std::sync::mpsc::channel::<()>();
        let request = {
            let slots = slots.clone();
            tokio::spawn(async move { run_bounded(&slots, Duration::from_secs(5), move || released.recv()).await })
        };
        while slots.available_permits() == 1 {
            tokio::task::yield_now().await;
        }

        request.abort();
        let _ = request.await;
        let while_running = run_bounded(&slots, Duration::from_millis(50), || ()).await;
        release.send(()).unwrap();
        let after = run_bounded(&slots, Duration::from_secs(5), || ()).await;

        assert!(matches!(while_running, Err(Busy)));
        assert!(after.is_ok());
    }
}
