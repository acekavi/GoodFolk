//! Guards that hold for every migration, including future ones.

use db::testing::app_pool;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// A table with a `tenant_id` column must have row-level security enabled and forced, or one tenant could
/// read another's rows -- except `guest_search`, which is deliberately exempt: it carries no data worth
/// protecting on its own (an id and a name), has no privileges for `goodfolk_app` at all (checked directly in
/// `crates/db/tests/guest_search.rs`), and is read only through `app.search_guest_ids`, a `SECURITY DEFINER`
/// function that enforces the tenant filter itself (`migrations/0009_guest_search.sql`).
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_tenant_table_has_forced_row_level_security(pool: PgPool) {
    let unprotected: Vec<String> = sqlx::query_scalar(
        "select c.relname::text
         from pg_class c
         join pg_namespace n on n.oid = c.relnamespace
         join pg_attribute a on a.attrelid = c.oid and a.attname = 'tenant_id' and not a.attisdropped
         where n.nspname = 'public' and c.relkind in ('r', 'p')
           and c.relname <> 'guest_search'
           and not (c.relrowsecurity and c.relforcerowsecurity)
         order by 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(unprotected.is_empty(), "tables without forced RLS: {unprotected:?}");
}

/// The API role must never own tables (owners bypass RLS) or bypass RLS itself.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_app_role_cannot_bypass_row_level_security(pool: PgPool) {
    let (bypass, owns): (bool, i64) = sqlx::query_as(
        "select r.rolbypassrls,
                (select count(*) from pg_class c where c.relowner = r.oid and c.relkind = 'r')
         from pg_roles r where r.rolname = 'goodfolk_app'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(!bypass);
    assert_eq!(owns, 0);
}

/// The outbox tables are reachable only through their security definer functions (`migrations/0014_outbox.sql`).
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_outbox_is_reachable_only_through_its_functions(pool: PgPool) {
    for table in ["outbox", "processed_event"] {
        for privilege in ["SELECT", "INSERT", "UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"] {
            let has: bool = sqlx::query_scalar("select has_table_privilege('goodfolk_app', $1, $2)")
                .bind(table)
                .bind(privilege)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(!has, "goodfolk_app has {privilege} on {table}");
        }
    }
    for function in [
        "app.outbox_enqueue(uuid, text, jsonb)",
        "app.outbox_claim(int, int)",
        "app.outbox_sent(uuid)",
        "app.outbox_retry(uuid, text, int)",
        "app.event_first_seen(text)",
        "app.outbox_refused(uuid, text)",
    ] {
        let can: bool =
            sqlx::query_scalar("select has_function_privilege('goodfolk_app', $1::regprocedure, 'execute')")
                .bind(function)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(can, "goodfolk_app cannot execute {function}");
    }
}

/// A security definer function that searches the session's temporary schema first can be hijacked by a temp table of
/// the same name: `pg_temp` must come last (`migrations/0015_definer_search_path.sql`).
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn definer_functions_search_pg_temp_last(pool: PgPool) {
    let bad: Vec<String> = sqlx::query_scalar(
        "select p.proname::text from pg_proc p join pg_namespace n on n.oid = p.pronamespace
         where n.nspname = 'app' and p.prosecdef
           and not coalesce(p.proconfig @> array['search_path=pg_catalog, public, pg_temp'], false)
         order by 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(bad.is_empty(), "security definer functions with a weak search_path: {bad:?}");
}

/// Money rows are append-only (`migrations/0020_folio.sql`): the application role holds no update or delete on a line,
/// a line's taxes or a payment, and may change an invoice only by recording where its PDF is stored. A folio changes
/// only its status and version, and is never deleted (`0029`).
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_app_role_cannot_rewrite_money_rows(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    for table in ["folio_line", "folio_line_tax", "payment", "invoice"] {
        for privilege in ["UPDATE", "DELETE"] {
            let has: bool = sqlx::query_scalar("select has_table_privilege('goodfolk_app', $1, $2)")
                .bind(table)
                .bind(privilege)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(!has, "goodfolk_app has table-wide {privilege} on {table}");
        }
    }
    let updatable: Vec<String> = sqlx::query_scalar(
        "select a.attname::text from pg_attribute a
         where a.attrelid = 'invoice'::regclass and a.attnum > 0 and not a.attisdropped
           and has_column_privilege('goodfolk_app', 'invoice', a.attname, 'UPDATE')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(updatable, ["pdf_key"]);
    for table in ["folio_line", "folio_line_tax", "payment", "invoice", "pos_bill"] {
        for privilege in ["TRUNCATE", "REFERENCES", "TRIGGER"] {
            let has: bool = sqlx::query_scalar("select has_table_privilege('goodfolk_app', $1, $2)")
                .bind(table)
                .bind(privilege)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(!has, "goodfolk_app has {privilege} on {table}");
        }
    }
    for privilege in ["UPDATE", "DELETE"] {
        let has: bool = sqlx::query_scalar("select has_table_privilege('goodfolk_app', 'pos_bill', $1)")
            .bind(privilege)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!has, "goodfolk_app has {privilege} on pos_bill");
    }
    let folio_updatable: Vec<String> = sqlx::query_scalar(
        "select a.attname::text from pg_attribute a
         where a.attrelid = 'folio'::regclass and a.attnum > 0 and not a.attisdropped
           and has_column_privilege('goodfolk_app', 'folio', a.attname, 'UPDATE')
         order by 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(folio_updatable, ["status", "version"]);
    let folio_deletable: bool = sqlx::query_scalar("select has_table_privilege('goodfolk_app', 'folio', 'DELETE')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!folio_deletable, "goodfolk_app can delete a folio");
    for table in ["folio_line", "folio_line_tax", "payment"] {
        let columns: i64 = sqlx::query_scalar(
            "select count(*) from pg_attribute a
             where a.attrelid = $1::regclass and a.attnum > 0 and not a.attisdropped
               and has_column_privilege('goodfolk_app', $1::regclass, a.attname, 'UPDATE')",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(columns, 0, "goodfolk_app can update a column of {table}");
    }
}
