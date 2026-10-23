//! The daily jobs: each runs once when due, under its lock, with its state in `job_state`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use common::hotel::Hotel;
use common::svc;
use jobs_svc::JobsSvc;
use jobs_svc::tick::tick_with;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use storage::{Bucket, Storage, StorageConfig};
use time::{Date, Duration, PrimitiveDateTime};
use uuid::Uuid;

/// The daily jobs' outcomes of one tick at local time `now`, as `(job, key, outcome)`.
async fn daily(svc: &JobsSvc, now: PrimitiveDateTime) -> Vec<(String, String, String)> {
    tick_with(svc, |_| now)
        .await
        .jobs
        .into_iter()
        .filter(|job| job.job != "auto_close" && job.job != "resume")
        .map(|job| (job.job, job.key, job.outcome))
        .collect()
}

fn ran(job: &str, key: &str) -> (String, String, String) {
    (job.to_owned(), key.to_owned(), "ran".to_owned())
}

/// `job`'s state for `key`: `(done_for, whether it has succeeded, last_error)`.
async fn state(owner: &PgPool, job: &str, key: &str) -> Option<(Option<Date>, bool, Option<String>)> {
    sqlx::query_as("select done_for, last_ok_at is not null, last_error from job_state where job = $1 and key = $2")
        .bind(job)
        .bind(key)
        .fetch_optional(owner)
        .await
        .unwrap()
}

/// At `hour`:`minute` on D + `days`, before D's audit moment while `days` is 0.
fn at(hotel: &Hotel, days: i64, hour: u8, minute: u8) -> PrimitiveDateTime {
    PrimitiveDateTime::new(hotel.day(days), time::Time::from_hms(hour, minute, 0).unwrap())
}

/// A rate source that quotes every currency asked for at 300, or leaves GBP out while `short`, and counts its calls.
#[derive(Default)]
struct FakeSource {
    short: AtomicBool,
    calls: AtomicU32,
}

#[async_trait::async_trait]
impl fx::RateSource for FakeSource {
    async fn tt_selling(
        &self,
        currencies: &[String],
        on: Date,
    ) -> Result<Vec<(String, Date, rust_decimal::Decimal)>, fx::FxError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let short = self.short.load(Ordering::SeqCst);
        Ok(currencies
            .iter()
            .filter(|c| !(short && *c == "GBP"))
            .map(|c| (c.clone(), on, rust_decimal::Decimal::from(300)))
            .collect())
    }
}

async fn storage() -> Arc<Storage> {
    let storage = Storage::new(StorageConfig::dev().unwrap()).unwrap();
    storage.ensure_dev_buckets().await.expect("start RustFS: deploy/dev/rustfs.sh start");
    Arc::new(storage)
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_window_cleanup_and_drift_jobs_run_once_a_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let svc = svc(hotel.pool.clone(), opts);
    let p = hotel.property.to_string();
    let last_day = async || -> Option<Date> {
        sqlx::query_scalar("select max(date) from inventory_day where property_id = $1")
            .bind(hotel.property)
            .fetch_one(&owner)
            .await
            .unwrap()
    };

    assert_eq!(daily(&svc, at(&hotel, 0, 12, 0)).await, vec![ran("cleanup", ""), ran("window", &p), ran("drift", &p)]);
    let window = last_day().await.expect("the window job extended the window");
    assert_eq!(state(&owner, "window", &p).await, Some((Some(hotel.day(0)), true, None)));
    assert_eq!(state(&owner, "drift", &p).await, Some((Some(hotel.day(0)), true, None)));
    assert_eq!(state(&owner, "cleanup", "").await, Some((Some(hotel.day(0)), true, None)));

    assert_eq!(daily(&svc, at(&hotel, 0, 18, 0)).await, vec![], "nothing is due again the same day");

    // A new business date makes the per-property jobs due; cleanup waits for the next calendar day.
    hotel.move_business_date(hotel.property, 1).await;
    assert_eq!(daily(&svc, at(&hotel, 0, 20, 0)).await, vec![ran("window", &p), ran("drift", &p)]);
    assert_eq!(last_day().await, Some(window + Duration::DAY));
    assert_eq!(state(&owner, "window", &p).await, Some((Some(hotel.day(1)), true, None)));
    assert_eq!(daily(&svc, at(&hotel, 1, 0, 0)).await, vec![ran("cleanup", "")]);
    assert_eq!(state(&owner, "cleanup", "").await, Some((Some(hotel.day(1)), true, None)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn two_concurrent_ticks_run_each_job_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let source = Arc::new(FakeSource::default());
    let svc =
        JobsSvc { storage: Some(storage().await), fx_source: Some(source.clone()), ..svc(hotel.pool.clone(), opts) };
    let now = at(&hotel, 0, 10, 30);

    let (first, second) = tokio::join!(daily(&svc, now), daily(&svc, now));
    let mut ran: Vec<(String, String)> = first
        .iter()
        .chain(&second)
        .filter(|(.., outcome)| outcome == "ran")
        .map(|(j, k, _)| (j.clone(), k.clone()))
        .collect();
    ran.sort();
    let p = hotel.property.to_string();
    let mut expected: Vec<(String, String)> =
        [("cleanup", ""), ("drift", p.as_str()), ("fx_import", ""), ("media_sweep", ""), ("window", p.as_str())]
            .map(|(j, k)| (j.to_owned(), k.to_owned()))
            .to_vec();
    expected.sort();
    assert_eq!(ran, expected, "{first:?} {second:?}");
    for (job, _, outcome) in first.iter().chain(&second) {
        assert!(outcome == "ran" || outcome == "skipped", "{job}: {outcome}");
    }
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_alerted_import_is_tried_again_and_a_complete_one_is_not(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let source = Arc::new(FakeSource::default());
    source.short.store(true, Ordering::SeqCst);
    let svc = JobsSvc { fx_source: Some(source.clone()), ..svc(hotel.pool.clone(), opts.clone()) };
    let fx = |jobs: Vec<(String, String, String)>| -> Vec<String> {
        jobs.into_iter().filter(|(job, ..)| job == "fx_import").map(|(.., outcome)| outcome).collect()
    };
    let alerted = "failed: 1 of 1 properties alerted; tried again at the next tick".to_owned();

    // The first import ever is due whatever the time; one that alerts is tried again at each tick.
    assert_eq!(fx(daily(&svc, at(&hotel, 0, 8, 0)).await), vec![alerted.clone()]);
    assert_eq!(state(&owner, "fx_import", "").await, Some((None, false, Some(alerted[8..].to_owned()))));
    assert_eq!(fx(daily(&svc, at(&hotel, 0, 8, 15)).await), vec![alerted.clone()]);
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);

    // A complete one before 10:30 leaves the day's import due at 10:30.
    source.short.store(false, Ordering::SeqCst);
    assert_eq!(fx(daily(&svc, at(&hotel, 0, 8, 30)).await), vec!["ran".to_owned()]);
    assert_eq!(state(&owner, "fx_import", "").await, Some((Some(hotel.day(-1)), true, None)));
    assert_eq!(fx(daily(&svc, at(&hotel, 0, 10, 29)).await), Vec::<String>::new());
    assert_eq!(fx(daily(&svc, at(&hotel, 0, 10, 30)).await), vec!["ran".to_owned()]);
    assert_eq!(state(&owner, "fx_import", "").await, Some((Some(hotel.day(0)), true, None)));
    assert_eq!(fx(daily(&svc, at(&hotel, 0, 23, 0)).await), Vec::<String>::new(), "a complete one is not repeated");
    assert_eq!(source.calls.load(Ordering::SeqCst), 4);

    // The next day's waits while another session holds the job's lock.
    let mut holder = PgConnection::connect_with(&opts).await.unwrap();
    sqlx::query("select pg_advisory_lock(hashtext('job:fx_import'))").execute(&mut holder).await.unwrap();
    assert_eq!(fx(daily(&svc, at(&hotel, 1, 10, 30)).await), vec!["skipped".to_owned()]);
    holder.close().await.unwrap();
    assert_eq!(fx(daily(&svc, at(&hotel, 1, 10, 45)).await), vec!["ran".to_owned()]);
    assert_eq!(source.calls.load(Ordering::SeqCst), 5);
}

#[sqlx::test(migrations = "../../migrations")]
async fn cleanup_deletes_exactly_the_expired_rows(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let svc = svc(hotel.pool.clone(), opts);
    for (token, expires) in [("expired", "-1 minute"), ("expired too", "-2 days"), ("fresh", "1 hour")] {
        sqlx::query(
            "insert into session (id, token_hash, user_id, expires_at)
             values (gen_random_uuid(), sha256($1::bytea), $2, now() + $3::interval)",
        )
        .bind(token.as_bytes())
        .bind(hotel.user.0)
        .bind(expires)
        .execute(&owner)
        .await
        .unwrap();
    }
    // The throttle window is 15 minutes.
    for age in ["16 minutes", "3 days", "14 minutes"] {
        sqlx::query(
            "insert into login_failure (id, email, at) values (gen_random_uuid(), 'u@example.com', now() - $1::interval)",
        )
        .bind(age)
        .execute(&owner)
        .await
        .unwrap();
    }
    for (key, age) in
        [("key-aged-8-days", "8 days"), ("key-just-past-a-week", "7 days 1 minute"), ("key-aged-6-days", "6 days")]
    {
        sqlx::query(
            "insert into idempotency_key (tenant_id, key, request_hash, created_at)
             values ($1, $2, '\\x01', now() - $3::interval)",
        )
        .bind(hotel.tenant.0)
        .bind(key)
        .bind(age)
        .execute(&owner)
        .await
        .unwrap();
    }
    let counts = async || -> (i64, i64, i64) {
        sqlx::query_as(
            "select (select count(*) from session), (select count(*) from login_failure),
                    (select count(*) from idempotency_key)",
        )
        .fetch_one(&owner)
        .await
        .unwrap()
    };
    assert_eq!(counts().await, (3, 3, 3));

    assert!(daily(&svc, at(&hotel, 0, 12, 0)).await.contains(&ran("cleanup", "")));
    assert_eq!(counts().await, (1, 1, 1));
    let left: Vec<String> = sqlx::query_scalar("select key from idempotency_key").fetch_all(&owner).await.unwrap();
    assert_eq!(left, ["key-aged-6-days"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_drift_raises_its_alert_and_a_clean_check_clears_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let svc = svc(hotel.pool.clone(), opts);
    hotel.window(hotel.property).await;
    let set_sold = async |sold: i32| {
        sqlx::query("update inventory_day set sold = $3 where property_id = $1 and date = $2")
            .bind(hotel.property)
            .bind(hotel.day(2))
            .bind(sold)
            .execute(&owner)
            .await
            .unwrap();
    };
    let alerts = async || -> Vec<(String, String)> {
        sqlx::query_as("select kind, message from ops_alert where property_id = $1")
            .bind(hotel.property)
            .fetch_all(&owner)
            .await
            .unwrap()
    };

    set_sold(1).await;
    let p = hotel.property.to_string();
    assert!(daily(&svc, at(&hotel, 0, 12, 0)).await.contains(&ran("drift", &p)));
    let message = format!("Inventory counters differ from the bookings on 1 days, from {}; see the logs", hotel.day(2));
    assert_eq!(alerts().await, vec![("drift".to_owned(), message)]);

    set_sold(0).await;
    hotel.move_business_date(hotel.property, 1).await;
    assert!(daily(&svc, at(&hotel, 1, 12, 0)).await.contains(&ran("drift", &p)));
    assert_eq!(alerts().await, vec![]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_media_sweep_removes_an_orphans_objects_once_a_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts.clone()).await;
    let storage = storage().await;
    let svc = JobsSvc { storage: Some(storage.clone()), ..svc(hotel.pool.clone(), opts.clone()) };
    // A ready gallery asset nothing links to, two days old: an orphan.
    let asset = Uuid::now_v7();
    let rendition = storage::rendition_key(hotel.tenant.0, asset, "md", "avif");
    let archive = storage::archive_key(hotel.tenant.0, asset, "jxl");
    let upload = storage::upload_key(hotel.tenant.0, asset);
    storage.put(Bucket::Public, &rendition, b"md".to_vec(), "image/avif", None).await.unwrap();
    storage.put(Bucket::Private, &archive, b"archive".to_vec(), "image/jxl", None).await.unwrap();
    storage.put(Bucket::Private, &upload, b"raw".to_vec(), "image/jpeg", None).await.unwrap();
    let renditions = serde_json::json!([{"size": "md", "format": "avif", "width": 320, "height": 240, "bytes": 2,
                                         "key": rendition}]);
    sqlx::query(
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, status, filename, declared_bytes,
                                  sha256, blake3, mime, bytes, archive_key, renditions, ready_at, created_at)
         values ($1, $2, $3, 'gallery', 'public', 'ready', 'file', 7, $4, $4, 'image/jpeg', 7, $5, $6, now(),
                 now() - interval '2 days')",
    )
    .bind(asset)
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(&[7u8; 32][..])
    .bind(&archive)
    .bind(renditions)
    .execute(&owner)
    .await
    .unwrap();
    let sweep = |jobs: Vec<(String, String, String)>| -> Vec<String> {
        jobs.into_iter().filter(|(job, ..)| job == "media_sweep").map(|(.., outcome)| outcome).collect()
    };

    // Another session holding the lock means another instance is sweeping.
    let mut holder = PgConnection::connect_with(&opts).await.unwrap();
    sqlx::query("select pg_advisory_lock(hashtext('job:media_sweep'))").execute(&mut holder).await.unwrap();
    assert_eq!(sweep(daily(&svc, at(&hotel, 0, 1, 0)).await), vec!["skipped".to_owned()]);
    assert!(storage.head(Bucket::Public, &rendition).await.unwrap().is_some());
    assert_eq!(state(&owner, "media_sweep", "").await, None);
    holder.close().await.unwrap();

    assert_eq!(sweep(daily(&svc, at(&hotel, 0, 1, 15)).await), vec!["ran".to_owned()]);
    assert_eq!(storage.head(Bucket::Public, &rendition).await.unwrap(), None);
    assert_eq!(storage.head(Bucket::Private, &archive).await.unwrap(), None);
    assert_eq!(storage.head(Bucket::Private, &upload).await.unwrap(), None);
    assert_eq!(state(&owner, "media_sweep", "").await, Some((Some(hotel.day(0)), true, None)));
    assert_eq!(sweep(daily(&svc, at(&hotel, 0, 23, 45)).await), Vec::<String>::new(), "once a day");
    assert_eq!(sweep(daily(&svc, at(&hotel, 1, 0, 0)).await), vec!["ran".to_owned()]);
}
