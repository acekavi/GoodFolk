//! A close stopped anywhere resumes to the same final state: for every step, a hook interrupts it once before its
//! commit (a crash mid-step) or after it (a crash between steps), and a second `execute` completes the run.

mod common;

use audit::{AuditError, AuditRun, CloseHooks, Mode, NoHooks, RunStatus, Step};
use common::{Hotel, Stay};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::atomic::{AtomicBool, Ordering};
use time::Date;
use uuid::Uuid;

/// Returns `Interrupted` once, before or after `step`'s commit.
struct Interrupt {
    step: Step,
    before: bool,
    fired: AtomicBool,
}

impl Interrupt {
    fn at(&self, step: Step, before: bool) -> Result<(), AuditError> {
        if step == self.step && before == self.before && !self.fired.swap(true, Ordering::SeqCst) {
            return Err(AuditError::Interrupted);
        }
        Ok(())
    }
}

impl CloseHooks for Interrupt {
    fn before_commit(&self, step: Step) -> Result<(), AuditError> {
        self.at(step, true)
    }

    fn after_commit(&self, step: Step) -> Result<(), AuditError> {
        self.at(step, false)
    }
}

/// What a close leaves behind, to compare before and after a rerun.
#[derive(Debug, PartialEq)]
struct State {
    business_date: Date,
    keys: Vec<String>,
    lines: i64,
    stats: i64,
    steps: i64,
    completions: i64,
}

impl Hotel {
    async fn state(&self) -> State {
        let mut tx = self.tx().await;
        let p = self.property;
        let one = |sql: &'static str| sqlx::query_scalar::<_, i64>(sql).bind(p);
        State {
            business_date: sqlx::query_scalar("select business_date from property where id = $1")
                .bind(p)
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
            keys: self.keyed_lines(p).await.into_iter().map(|(key, _)| key).collect(),
            lines: one("select count(*) from folio_line l join folio f on f.id = l.folio_id where f.property_id = $1")
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
            stats: one("select count(*) from daily_stats where property_id = $1").fetch_one(&mut *tx).await.unwrap(),
            steps: one(
                "select count(*) from audit_run_step s join audit_run r on r.id = s.run_id where r.property_id = $1",
            )
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
            completions: one("select count(*) from audit_log l join audit_run r on r.id = l.entity_id
                 where r.property_id = $1 and l.action = 'night_audit.completed'")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        }
    }

    async fn run(&self, run: Uuid) -> AuditRun {
        audit::get_run(&mut self.tx().await, run).await.unwrap()
    }
}

/// A property with something for every step: an arrival to mark a no-show (its fee posts), a departure to extend,
/// a stay in house in LKR and one in USD (posted at D − 1's rate, which `lock_fx` stores as D's).
async fn hotel(opts: PgConnectOptions) -> (Hotel, Vec<String>) {
    let hotel = Hotel::new(opts).await;
    let p = hotel.property;
    let d = hotel.business_date;
    let terms = serde_json::json!({ "rules": [], "no_show": { "kind": "nights", "value": 1 } });
    let arrival = hotel.stay(Stay { room: Some("101"), terms: Some(terms), ..Stay::new(p, "confirmed", 0, 2) }).await;
    let leaving = hotel.stay(Stay { room: Some("102"), ..Stay::new(p, "checked_in", -1, 0) }).await;
    let lkr = hotel.stay(Stay { room: Some("103"), ..Stay::new(p, "checked_in", -1, 2) }).await;
    let usd = hotel
        .stay(Stay { room: Some("104"), currency: "USD", room_amount: 12_000, ..Stay::new(p, "checked_in", -1, 2) })
        .await;
    hotel.room(p, "105", true).await;
    hotel.window(p).await;
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
         values ($1, $2, $3, 'USD', 300, 'cbsl', $3)",
    )
    .bind(hotel.tenant.0)
    .bind(p)
    .bind(hotel.day(-1))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut keys = vec![
        format!("no_show:{}", arrival.id),
        format!("room:{}:{d}", leaving.id),
        format!("room:{}:{d}", lkr.id),
        format!("room:{}:{d}", usd.id),
    ];
    keys.sort();
    (hotel, keys)
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_close_interrupted_at_any_point_resumes_to_the_same_end(_: PgPoolOptions, opts: PgConnectOptions) {
    for step in Step::ALL {
        for before in [true, false] {
            let case = format!("{} {}", if before { "before" } else { "after" }, step.as_str());
            let (hotel, keys) = hotel(opts.clone()).await;
            let d = hotel.business_date;
            let run = audit::start_run(&hotel.pool, hotel.tenant, None, hotel.property, Mode::Auto).await.unwrap();
            let hooks = Interrupt { step, before, fired: AtomicBool::new(false) };

            let interrupted = audit::execute(&hotel.pool, &opts, hotel.tenant, run.id, &hooks).await.unwrap_err();
            assert!(matches!(interrupted, AuditError::Interrupted), "{case}: {interrupted:?}");
            let stopped = hotel.run(run.id).await;
            let position = Step::ALL.iter().position(|s| *s == step).unwrap();
            // The roll's two steps commit together, with the run's completion.
            let rolled = !before && position >= 3;
            if rolled {
                assert_eq!(stopped.status, RunStatus::Completed, "{case}");
            } else {
                assert_eq!(stopped.status, RunStatus::Running, "{case}");
                let done = if before { position.min(3) } else { position + 1 };
                assert_eq!(stopped.steps.len(), done, "{case}");
                // Starting D's close again while it is running returns the same run.
                let again =
                    audit::start_run(&hotel.pool, hotel.tenant, None, hotel.property, Mode::Auto).await.unwrap();
                assert_eq!(again, stopped, "{case}");
            }

            let completed = audit::execute(&hotel.pool, &opts, hotel.tenant, run.id, &NoHooks).await.unwrap().unwrap();
            assert_eq!(completed.status, RunStatus::Completed, "{case}: {:?}", completed.error);
            assert_eq!(completed.attempts, 1, "{case}");
            let state = hotel.state().await;
            assert_eq!(
                state,
                State { business_date: hotel.day(1), keys: keys.clone(), lines: 4, stats: 1, steps: 5, completions: 1 },
                "{case}"
            );
            assert_eq!(completed.business_date, d);

            // A further execute of the completed run changes nothing.
            let replay = audit::execute(&hotel.pool, &opts, hotel.tenant, run.id, &NoHooks).await.unwrap().unwrap();
            assert_eq!(replay, completed, "{case}");
            assert_eq!(hotel.state().await, state, "{case}");
        }
    }
}
