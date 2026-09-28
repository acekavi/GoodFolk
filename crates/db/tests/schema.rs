//! Guards that hold for every migration, including future ones.

use sqlx::PgPool;

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
