//! The night-audit close (`audit::start_run`, `audit::retry_run`, `audit::execute`) and its ops alert, on stays seeded
//! straight into the reservation tables.

mod common;

use audit::{AlertKind, AuditError, AuditRun, CloseHooks, Mode, NoHooks, RunStatus, Step};
use common::{Hotel, Stay};
use db::{CHANNEL, Event};
use rust_decimal::Decimal;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use sqlx::{Connection, PgConnection};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use time::Date;
use uuid::Uuid;

impl Hotel {
    async fn start(&self, mode: Mode) -> Result<AuditRun, AuditError> {
        let user = (mode == Mode::Manual).then_some(self.user);
        audit::start_run(&self.pool, self.tenant, user, self.property, mode).await
    }

    async fn execute(&self, opts: &PgConnectOptions, run: Uuid) -> AuditRun {
        audit::execute(&self.pool, opts, self.tenant, run, &NoHooks).await.unwrap().unwrap()
    }

    async fn retry(&self, run: Uuid) -> AuditRun {
        audit::retry_run(&self.pool, self.tenant, Some(self.user), run).await.unwrap()
    }

    async fn current_date(&self) -> Date {
        sqlx::query_scalar("select business_date from property where id = $1")
            .bind(self.property)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    async fn runs(&self) -> i64 {
        sqlx::query_scalar("select count(*) from audit_run").fetch_one(&mut *self.tx().await).await.unwrap()
    }

    /// The actions audited on `run`, oldest first.
    async fn audited(&self, run: Uuid) -> Vec<String> {
        sqlx::query_scalar("select action from audit_log where entity_id = $1 order by at, id")
            .bind(run)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// `(status, check-out)` of the stay `id`.
    async fn stay_row(&self, id: Uuid) -> (String, Date) {
        sqlx::query_as("select status, upper(stay) from reservation_room where id = $1")
            .bind(id)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    async fn alerts(&self) -> Vec<AlertKind> {
        let alerts = audit::alerts::list(&mut self.tx().await, self.property).await.unwrap();
        alerts.into_iter().map(|alert| alert.kind).collect()
    }

    async fn listener(&self) -> PgListener {
        let mut listener = PgListener::connect_with(&self.pool).await.unwrap();
        listener.listen(CHANNEL).await.unwrap();
        listener
    }
}

/// Every event's keys until none arrives for a second.
async fn drain(listener: &mut PgListener) -> Vec<Vec<String>> {
    let mut events = Vec::new();
    while let Ok(received) = tokio::time::timeout(Duration::from_secs(1), listener.recv()).await {
        let event: Event = serde_json::from_str(received.unwrap().payload()).unwrap();
        events.push(event.keys);
    }
    events
}

fn steps(run: &AuditRun) -> Vec<Step> {
    run.steps.iter().map(|step| step.step).collect()
}

fn detail(run: &AuditRun, step: Step) -> serde_json::Value {
    run.steps.iter().find(|done| done.step == step).unwrap().detail.clone()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_manual_close_runs_every_step_and_rolls_the_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let p = hotel.property;
    let d = hotel.business_date;
    let stay = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", -1, 2) }).await;
    // USD's rate in effect on D is D − 1's: lock_fx stores it as D's.
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
         values ($1, $2, $3, 'USD', 300, 'cbsl', $4)",
    )
    .bind(hotel.tenant.0)
    .bind(p)
    .bind(hotel.day(-1))
    .bind(hotel.day(-3))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut listener = hotel.listener().await;

    let started = hotel.start(Mode::Manual).await.unwrap();
    assert_eq!((started.status, started.mode, started.attempts), (RunStatus::Running, Mode::Manual, 1));
    assert_eq!((started.business_date, started.started_by), (d, Some(hotel.user.0)));
    assert!(started.steps.is_empty());
    let run = hotel.execute(&opts, started.id).await;

    assert_eq!(run.status, RunStatus::Completed);
    assert!(run.finished_at.is_some() && run.error.is_none());
    assert_eq!(steps(&run), Step::ALL);
    assert_eq!(detail(&run, Step::Resolve), serde_json::json!({ "no_shows": 0, "cancelled": 0, "extended": 0 }));
    assert_eq!(detail(&run, Step::PostCharges), serde_json::json!({ "stays": 1, "lines": 1, "skipped_existing": 0 }));
    assert_eq!(detail(&run, Step::LockFx)["locked"][0]["currency"], "USD");
    assert_eq!(hotel.current_date().await, hotel.day(1));
    assert_eq!(hotel.keyed_lines(p).await, vec![(format!("room:{}:{d}", stay.id), 10_000)]);
    let stats = audit::daily_stats(&mut hotel.tx().await, p, d, d).await.unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].rooms_sold, 1);
    let locked: (Date, String) =
        sqlx::query_as("select source_date, source from fx_rate where property_id = $1 and date = $2")
            .bind(p)
            .bind(d)
            .fetch_one(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(locked, (hotel.day(-3), "cbsl".into()));
    assert_eq!(hotel.audited(run.id).await, ["night_audit.started", "night_audit.completed"]);

    let events = drain(&mut listener).await;
    let audit_key = audit::audit_key(p);
    assert_eq!(events[0], std::slice::from_ref(&audit_key), "the start");
    let roll = events.last().unwrap();
    for key in [audit_key, property::PROPERTIES_KEY.to_string(), reservations::reservations_key(p)] {
        assert!(roll.contains(&key), "{key} in {roll:?}");
    }

    // Starting D's close again returns it; the next start is D + 1's.
    let mut tx = hotel.tx().await;
    assert_eq!(audit::get_run(&mut tx, run.id).await.unwrap(), run);
    drop(tx);
    let next = hotel.start(Mode::Manual).await.unwrap();
    assert_eq!(next.business_date, hotel.day(1));
    let listed = audit::list_runs(&mut hotel.tx().await, p, 10, None).await.unwrap();
    assert_eq!(listed.iter().map(|run| run.id).collect::<Vec<_>>(), [next.id, run.id]);
    let older = audit::list_runs(&mut hotel.tx().await, p, 10, Some(hotel.day(1))).await.unwrap();
    assert_eq!(older, [run]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_manual_close_waits_for_every_arrival_and_departure(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.stay(Stay::new(hotel.property, "confirmed", 0, 2)).await;

    let refused = hotel.start(Mode::Manual).await.unwrap_err();
    assert!(
        matches!(&refused, AuditError::Conflict(message) if message == "Resolve every arrival and departure first (1 left)"),
        "{refused:?}"
    );
    assert_eq!(hotel.runs().await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_auto_close_resolves_arrivals_and_departures(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let p = hotel.property;
    let d = hotel.business_date;
    let confirmed = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "confirmed", 0, 2) }).await;
    let tentative = hotel.stay(Stay { room: Some("102"), ..Stay::new(p, "tentative", 0, 1) }).await;
    let leaving = hotel.stay(Stay { room: Some("103"), ..Stay::new(p, "checked_in", -1, 0) }).await;
    hotel.window(p).await;

    let started = hotel.start(Mode::Auto).await.unwrap();
    assert_eq!(started.started_by, None);
    let run = hotel.execute(&opts, started.id).await;

    assert_eq!(run.status, RunStatus::Completed, "{:?}", run.error);
    assert_eq!(detail(&run, Step::Resolve), serde_json::json!({ "no_shows": 1, "cancelled": 1, "extended": 1 }));
    assert_eq!(hotel.stay_row(confirmed.id).await, ("no_show".into(), hotel.day(2)));
    assert_eq!(hotel.stay_row(tentative.id).await.0, "cancelled");
    assert_eq!(hotel.stay_row(leaving.id).await, ("checked_in".into(), hotel.day(1)));
    for (action, stay) in [("reservation_room.no_show", confirmed.id), ("reservation_room.cancelled", tentative.id)] {
        let mode: Option<String> =
            sqlx::query_scalar("select data ->> 'mode' from audit_log where action = $1 and entity_id = $2")
                .bind(action)
                .bind(stay)
                .fetch_one(&mut *hotel.tx().await)
                .await
                .unwrap();
        assert_eq!(mode.as_deref(), Some("auto"), "{action}");
    }
    // The extended night is D's, posted by the same close.
    assert_eq!(hotel.keyed_lines(p).await, vec![(format!("room:{}:{d}", leaving.id), 10_000)]);
    assert_eq!(hotel.current_date().await, hotel.day(1));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_auto_close_fails_on_a_stay_overdue_from_an_earlier_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let overdue = hotel.stay(Stay { room: Some("101"), ..Stay::new(hotel.property, "checked_in", -2, -1) }).await;

    let started = hotel.start(Mode::Auto).await.unwrap();
    let run = hotel.execute(&opts, started.id).await;

    assert_eq!(run.status, RunStatus::Failed);
    assert!(run.finished_at.is_some());
    let error = run.error.clone().unwrap();
    assert!(error.starts_with("Room 101: "), "{error}");
    assert_eq!(run.failure.as_ref().unwrap()["step"], "resolve");
    assert_eq!(run.failure.as_ref().unwrap()["stays"][0]["reservation_room"], overdue.id.to_string());
    assert!(run.steps.is_empty());
    assert_eq!(hotel.current_date().await, hotel.business_date);
    assert_eq!(hotel.audited(run.id).await, ["night_audit.started", "night_audit.failed"]);
    // A failed run is returned as it is, until retried.
    assert_eq!(hotel.execute(&opts, run.id).await, run);
    assert_eq!(hotel.start(Mode::Auto).await.unwrap(), run);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_auto_extension_into_a_taken_room_fails_the_run(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let p = hotel.property;
    let leaving = hotel.stay(Stay { room: Some("102"), ..Stay::new(p, "checked_in", -1, 0) }).await;
    // Another guest checked into 102 today.
    let next = hotel.stay(Stay::new(p, "checked_in", 0, 2)).await;
    let mut tx = hotel.tx().await;
    sqlx::query(
        "update reservation_room set room_id = (select room_id from reservation_room where id = $2) where id = $1",
    )
    .bind(next.id)
    .bind(leaving.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    hotel.room(p, "103", true).await;
    hotel.window(p).await;

    let started = hotel.start(Mode::Auto).await.unwrap();
    let run = hotel.execute(&opts, started.id).await;

    assert_eq!(run.status, RunStatus::Failed);
    let error = run.error.clone().unwrap();
    assert!(error.starts_with("Room 102: "), "{error}");
    assert_eq!(hotel.stay_row(leaving.id).await, ("checked_in".into(), hotel.business_date));
    assert_eq!(hotel.current_date().await, hotel.business_date);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_missing_rate_fails_the_posting_and_a_retry_posts_only_what_is_missing(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts.clone()).await;
    let p = hotel.property;
    let d = hotel.business_date;
    let lkr = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", -1, 1) }).await;
    let usd = hotel
        .stay(Stay { room: Some("102"), currency: "USD", room_amount: 12_000, ..Stay::new(p, "checked_in", -1, 1) })
        .await;

    let started = hotel.start(Mode::Manual).await.unwrap();
    let failed = hotel.execute(&opts, started.id).await;

    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("1 stays could not be posted"));
    let failure = failed.failure.clone().unwrap();
    assert_eq!(failure["step"], "post_charges");
    assert_eq!(failure["stays"].as_array().unwrap().len(), 1);
    assert_eq!(failure["stays"][0]["reservation_room"], usd.id.to_string());
    assert_eq!(failure["stays"][0]["room"], "102");
    assert!(failure["stays"][0]["error"].as_str().unwrap().contains("USD"), "{failure}");
    assert_eq!(steps(&failed), [Step::Resolve]);
    assert_eq!(hotel.keyed_lines(p).await, vec![(format!("room:{}:{d}", lkr.id), 10_000)]);

    // Only a failed run is retried.
    let refused = audit::retry_run(&hotel.pool, hotel.tenant, Some(hotel.user), Uuid::now_v7()).await.unwrap_err();
    assert!(matches!(refused, AuditError::NotFound(_)), "{refused:?}");
    hotel.rate(p, "USD", Decimal::new(300, 0)).await;
    let retried = hotel.retry(failed.id).await;
    assert_eq!((retried.status, retried.attempts, retried.error, retried.failure), (RunStatus::Running, 2, None, None));
    assert_eq!(retried.finished_at, None);
    let again = audit::retry_run(&hotel.pool, hotel.tenant, Some(hotel.user), failed.id).await.unwrap_err();
    assert!(matches!(again, AuditError::Conflict(_)), "{again:?}");
    let run = hotel.execute(&opts, failed.id).await;

    assert_eq!(run.status, RunStatus::Completed);
    assert_eq!(run.attempts, 2);
    assert_eq!(detail(&run, Step::PostCharges), serde_json::json!({ "stays": 2, "lines": 1, "skipped_existing": 1 }));
    let mut expected = vec![(format!("room:{}:{d}", lkr.id), 10_000), (format!("room:{}:{d}", usd.id), 12_000)];
    expected.sort();
    assert_eq!(hotel.keyed_lines(p).await, expected);
    assert_eq!(
        hotel.audited(run.id).await,
        ["night_audit.started", "night_audit.failed", "night_audit.retried", "night_audit.completed"]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_auto_run_failing_its_third_attempt_raises_the_alert_until_it_completes(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts.clone()).await;
    let p = hotel.property;
    hotel.stay(Stay { room: Some("101"), currency: "USD", ..Stay::new(p, "checked_in", -1, 1) }).await;

    let run = hotel.start(Mode::Auto).await.unwrap();
    for attempt in 1..=3 {
        if attempt > 1 {
            hotel.retry(run.id).await;
        }
        let failed = hotel.execute(&opts, run.id).await;
        assert_eq!((failed.status, failed.attempts), (RunStatus::Failed, attempt));
        let raised = if attempt == 3 { vec![AlertKind::Audit] } else { vec![] };
        assert_eq!(hotel.alerts().await, raised, "attempt {attempt}");
    }
    let alert = &audit::alerts::list(&mut hotel.tx().await, p).await.unwrap()[0];
    assert!(alert.message.contains("1 stays could not be posted"), "{}", alert.message);

    hotel.rate(p, "USD", Decimal::new(300, 0)).await;
    hotel.retry(run.id).await;
    let completed = hotel.execute(&opts, run.id).await;
    assert_eq!((completed.status, completed.attempts), (RunStatus::Completed, 4));
    assert_eq!(hotel.alerts().await, vec![]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_starts_make_one_run_and_concurrent_executes_post_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let p = hotel.property;
    for room in ["101", "102", "103"] {
        hotel.stay(Stay { room: Some(room), ..Stay::new(p, "checked_in", -1, 2) }).await;
    }

    let (a, b) = tokio::join!(hotel.start(Mode::Manual), hotel.start(Mode::Manual));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.id, b.id);
    assert_eq!(hotel.runs().await, 1);

    let (x, y) = tokio::join!(
        audit::execute(&hotel.pool, &opts, hotel.tenant, a.id, &NoHooks),
        audit::execute(&hotel.pool, &opts, hotel.tenant, a.id, &NoHooks),
    );
    let mut outcomes = [x.unwrap(), y.unwrap()];
    outcomes.sort_by_key(Option::is_some);
    assert_eq!(outcomes[0], None, "one execute finds the lock taken");
    assert_eq!(outcomes[1].as_ref().unwrap().status, RunStatus::Completed);
    assert_eq!(hotel.keyed_lines(p).await.len(), 3);
    assert_eq!(hotel.current_date().await, hotel.day(1));
}

/// Pauses the roll before its commit until the test lets it go.
struct PauseRoll {
    paused: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl CloseHooks for PauseRoll {
    fn before_commit(&self, step: Step) -> Result<(), AuditError> {
        if step == Step::SnapshotStats {
            self.paused.wait();
            self.release.wait();
        }
        Ok(())
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_cancellation_racing_the_roll_counts_on_exactly_one_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let p = hotel.property;
    let stay = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "confirmed", 3, 5) }).await;
    hotel.window(p).await;
    let run = hotel.start(Mode::Manual).await.unwrap();

    // The hook blocks its thread, so the close runs on a runtime of its own.
    let (paused, release) = (Arc::new(Barrier::new(2)), Arc::new(Barrier::new(2)));
    let hooks = PauseRoll { paused: paused.clone(), release: release.clone() };
    let (close_opts, tenant) = (opts.clone(), hotel.tenant);
    let close = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let pool = db::testing::app_pool(close_opts.clone(), 2).await;
            audit::execute(&pool, &close_opts, tenant, run.id, &hooks).await.unwrap().unwrap()
        })
    });
    tokio::task::spawn_blocking(move || paused.wait()).await.unwrap();

    let (pool, version) = (hotel.pool.clone(), 1);
    let cancel = tokio::spawn(async move {
        let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
        reservations::cancel_room(&mut tx, tenant, None, p, stay.id, version).await.unwrap();
        tx.commit().await.unwrap();
    });
    // The cancellation waits on the roll's lock on the property.
    let mut owner = PgConnection::connect_with(&opts).await.unwrap();
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "select count(*) from pg_stat_activity where datname = current_database() and wait_event_type = 'Lock'",
        )
        .fetch_one(&mut owner)
        .await
        .unwrap();
        if waiting > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::task::spawn_blocking(move || release.wait()).await.unwrap();
    let closed = tokio::task::spawn_blocking(move || close.join().unwrap()).await.unwrap();
    cancel.await.unwrap();

    assert_eq!(closed.status, RunStatus::Completed);
    let on: Date = sqlx::query_scalar("select cancelled_business_date from reservation_room where id = $1")
        .bind(stay.id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    let stored = audit::daily_stats(&mut hotel.tx().await, p, hotel.business_date, hotel.business_date).await.unwrap();
    let next = audit::compute(&mut hotel.tx().await, p, hotel.day(1)).await.unwrap();
    assert_eq!(stored[0].cancellations + next.cancellations, 1, "counted on exactly one day");
    assert_eq!(on, hotel.day(1), "the roll held the cancellation back");
    assert_eq!(next.cancellations, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_closed_day_s_rate_cannot_be_overridden(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let run = hotel.start(Mode::Manual).await.unwrap();
    hotel.execute(&opts, run.id).await;

    let mut tx = hotel.tx().await;
    let refused = fx::set_manual(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        hotel.business_date,
        "USD",
        Decimal::new(300, 0),
    )
    .await
    .unwrap_err();
    assert!(matches!(&refused, fx::FxError::Conflict(message) if message == "That day is closed"), "{refused:?}");
}

/// Pauses after `step`'s commit until the test lets it go.
struct PauseAfter {
    step: Step,
    paused: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl CloseHooks for PauseAfter {
    fn after_commit(&self, step: Step) -> Result<(), AuditError> {
        if step == self.step {
            self.paused.wait();
            self.release.wait();
        }
        Ok(())
    }
}

/// A stay checked in after `post_charges` and before the roll has no charge for D: the roll fails naming it, rather
/// than snapshot D without its night, and the retry posts the night before rolling.
#[sqlx::test(migrations = "../../migrations")]
async fn a_stay_checked_in_after_the_posting_fails_the_roll_until_retried(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (p, d) = (hotel.property, hotel.business_date);
    hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", -1, 2) }).await;
    let run = hotel.start(Mode::Manual).await.unwrap();

    let (paused, release) = (Arc::new(Barrier::new(2)), Arc::new(Barrier::new(2)));
    let hooks = PauseAfter { step: Step::LockFx, paused: paused.clone(), release: release.clone() };
    let (close_opts, tenant) = (opts.clone(), hotel.tenant);
    let close = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let pool = db::testing::app_pool(close_opts.clone(), 2).await;
            audit::execute(&pool, &close_opts, tenant, run.id, &hooks).await.unwrap().unwrap()
        })
    });
    tokio::task::spawn_blocking(move || paused.wait()).await.unwrap();
    let late = hotel.stay(Stay { room: Some("102"), ..Stay::new(p, "checked_in", 0, 2) }).await;
    tokio::task::spawn_blocking(move || release.wait()).await.unwrap();
    let failed = tokio::task::spawn_blocking(move || close.join().unwrap()).await.unwrap();

    assert_eq!(failed.status, RunStatus::Failed);
    let error = failed.error.clone().unwrap();
    assert!(error.contains("Room 102") && !error.contains("Room 101"), "{error}");
    let failure = failed.failure.clone().unwrap();
    assert_eq!(failure["step"], "snapshot_stats");
    assert_eq!(failure["stays"][0]["reservation_room"], late.id.to_string());
    assert_eq!(hotel.current_date().await, d);

    hotel.retry(run.id).await;
    let completed = hotel.execute(&opts, run.id).await;

    assert_eq!(completed.status, RunStatus::Completed, "{:?}", completed.error);
    let stats = audit::daily_stats(&mut hotel.tx().await, p, d, d).await.unwrap();
    assert_eq!((stats[0].rooms_sold, stats[0].room_revenue), (2, 20_000));
}

/// Stops a run after `post_charges` commits, as if the process died there.
struct DieAfterPosting;

impl CloseHooks for DieAfterPosting {
    fn after_commit(&self, step: Step) -> Result<(), AuditError> {
        if step == Step::PostCharges { Err(AuditError::Interrupted) } else { Ok(()) }
    }
}

/// A run resumed after `post_charges` posts D again before rolling, so a stay checked in meanwhile counts in D.
#[sqlx::test(migrations = "../../migrations")]
async fn a_resumed_run_posts_a_stay_checked_in_since(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (p, d) = (hotel.property, hotel.business_date);
    let early = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", -1, 2) }).await;
    let run = hotel.start(Mode::Manual).await.unwrap();
    let died = audit::execute(&hotel.pool, &opts, hotel.tenant, run.id, &DieAfterPosting).await.unwrap_err();
    assert!(matches!(died, AuditError::Interrupted), "{died:?}");

    let late = hotel.stay(Stay { room: Some("102"), ..Stay::new(p, "checked_in", 0, 2) }).await;
    let completed = hotel.execute(&opts, run.id).await;

    assert_eq!(completed.status, RunStatus::Completed, "{:?}", completed.error);
    let mut expected = vec![(format!("room:{}:{d}", early.id), 10_000), (format!("room:{}:{d}", late.id), 10_000)];
    expected.sort();
    assert_eq!(hotel.keyed_lines(p).await, expected);
    let stats = audit::daily_stats(&mut hotel.tx().await, p, d, d).await.unwrap();
    assert_eq!((stats[0].rooms_sold, stats[0].room_revenue), (2, 20_000));
}

/// Fails a step's commit with a database error whose text names a table.
struct DatabaseFails(fn() -> AuditError);

impl CloseHooks for DatabaseFails {
    fn before_commit(&self, _: Step) -> Result<(), AuditError> {
        Err((self.0)())
    }
}

fn leak() -> sqlx::Error {
    sqlx::Error::Protocol("relation \"audit_run_step\" does not exist".into())
}

/// A database error, however nested, is logged; the run, its audit entry and the alert say only that one stopped it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_database_error_reaches_staff_without_its_text(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let run = hotel.start(Mode::Auto).await.unwrap();
    let errors: [fn() -> AuditError; 3] = [
        || AuditError::Database(leak()),
        || AuditError::Folio(folio::FolioError::Database(leak())),
        || AuditError::Reservations(reservations::ReservationsError::Database(leak())),
    ];
    let expected = "A database error stopped the close; see the logs";
    for (attempt, error) in errors.into_iter().enumerate() {
        if attempt > 0 {
            hotel.retry(run.id).await;
        }
        let failed =
            audit::execute(&hotel.pool, &opts, hotel.tenant, run.id, &DatabaseFails(error)).await.unwrap().unwrap();
        assert_eq!((failed.status, failed.error.as_deref()), (RunStatus::Failed, Some(expected)), "{attempt}");
    }
    let audited: Vec<String> = sqlx::query_scalar(
        "select data ->> 'error' from audit_log where entity_id = $1 and action = 'night_audit.failed'",
    )
    .bind(run.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(audited, [expected; 3]);
    let alert = &audit::alerts::list(&mut hotel.tx().await, hotel.property).await.unwrap()[0];
    assert!(alert.message.ends_with(expected) && !alert.message.contains("relation"), "{}", alert.message);
}
