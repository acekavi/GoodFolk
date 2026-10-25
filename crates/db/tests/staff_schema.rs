//! What the Phase 8a migration (`migrations/0035_staff_settings.sql`) puts on invitations and the `pos` role, checked
//! directly in the database.

use db::testing::app_pool;
use db::{Scope, TenantId, begin};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use uuid::Uuid;

const PERMISSION_DENIED: &str = "42501";
const UNIQUE_VIOLATION: &str = "23505";

struct Hotel {
    tenant: Uuid,
    inviter: Uuid,
}

/// A tenant named "Palm Hotel" and the user who invites, "Inviter". Runs as the owner (no RLS).
async fn hotel(pool: &PgPool, name: &str) -> Hotel {
    let hotel = Hotel { tenant: Uuid::now_v7(), inviter: Uuid::now_v7() };
    sqlx::query("insert into tenant (id, name) values ($1, $2)")
        .bind(hotel.tenant)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'Inviter')")
        .bind(hotel.inviter)
        .bind(format!("{}@example.com", hotel.inviter))
        .execute(pool)
        .await
        .unwrap();
    hotel
}

/// Inserts a pending invitation for `email` (`expires` an interval from now) and returns its id and token hash.
async fn invite(pool: &PgPool, hotel: &Hotel, email: &str, expires: &str) -> (Uuid, Vec<u8>) {
    let id = Uuid::now_v7();
    let hash = Uuid::now_v7().as_bytes().to_vec();
    sqlx::query(
        "insert into invitation (id, tenant_id, email, display_name, grants, token_hash, status, invited_by, expires_at)
         values ($1, $2, $3, 'New', '[{\"role\": \"pos\"}]', $4, 'pending', $5, now() + $6::interval)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(email)
    .bind(&hash)
    .bind(hotel.inviter)
    .bind(expires)
    .execute(pool)
    .await
    .unwrap();
    (id, hash)
}

fn sqlstate(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|e| e.code()).map(|c| c.into_owned()).unwrap_or_default()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_pos_role_can_be_granted(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner, "Palm Hotel").await;
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(hotel.tenant)
        .bind(hotel.inviter)
        .execute(&owner)
        .await
        .unwrap();

    let granted =
        sqlx::query("insert into role_grant (id, tenant_id, user_id, role) values (gen_random_uuid(), $1, $2, 'pos')")
            .bind(hotel.tenant)
            .bind(hotel.inviter)
            .execute(&owner)
            .await;

    granted.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_membership_starts_at_version_one(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner, "Palm Hotel").await;
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(hotel.tenant)
        .bind(hotel.inviter)
        .execute(&owner)
        .await
        .unwrap();

    let version: i32 = sqlx::query_scalar("select version from membership").fetch_one(&owner).await.unwrap();

    assert_eq!(version, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_tenant_has_one_pending_invitation_per_email(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner, "Palm Hotel").await;
    let (first, _) = invite(&owner, &hotel, "Kamala@example.com", "7 days").await;

    let second = sqlx::query(
        "insert into invitation (id, tenant_id, email, display_name, grants, token_hash, status, expires_at)
         values (gen_random_uuid(), $1, 'kamala@EXAMPLE.com', 'Again', '[{\"role\": \"pos\"}]', '\\x01', 'pending',
                 now() + interval '7 days')",
    )
    .bind(hotel.tenant)
    .execute(&owner)
    .await;
    sqlx::query("update invitation set status = 'revoked' where id = $1").bind(first).execute(&owner).await.unwrap();
    let after_revoke = sqlx::query(
        "insert into invitation (id, tenant_id, email, display_name, grants, token_hash, status, expires_at)
         values (gen_random_uuid(), $1, 'kamala@example.com', 'Again', '[{\"role\": \"pos\"}]', '\\x02', 'pending',
                 now() + interval '7 days')",
    )
    .bind(hotel.tenant)
    .execute(&owner)
    .await;

    assert_eq!(sqlstate(second), UNIQUE_VIOLATION);
    after_revoke.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_app_role_cannot_delete_an_invitation(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    let hotel = hotel(&owner, "Palm Hotel").await;
    invite(&owner, &hotel, "kamala@example.com", "7 days").await;

    let mut tx = begin(&pool, Scope::tenant(TenantId(hotel.tenant))).await.unwrap();
    let deleted = sqlx::query("delete from invitation").execute(&mut *tx).await;

    assert_eq!(sqlstate(deleted), PERMISSION_DENIED);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn invitations_are_found_by_token_across_tenants_and_by_id_in_their_own(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    let other = hotel(&owner, "Other Hotel").await;
    let hotel = hotel(&owner, "Palm Hotel").await;
    let (id, hash) = invite(&owner, &hotel, "kamala@example.com", "7 days").await;

    // The app role has no tenant here, as when a person opens the link.
    let by_token = sqlx::query("select * from app.invitation_by_token($1)").bind(&hash).fetch_one(&pool).await.unwrap();
    let unknown = sqlx::query("select * from app.invitation_by_token($1)")
        .bind(vec![0u8; 32])
        .fetch_optional(&pool)
        .await
        .unwrap();
    let by_id = |tenant: Option<Uuid>| {
        let pool = pool.clone();
        async move {
            let scope = Scope { tenant: tenant.map(TenantId), user: None };
            let mut tx = begin(&pool, scope).await.unwrap();
            let row = sqlx::query("select * from app.invitation_by_id($1)").bind(id).fetch_optional(&mut *tx).await;
            tx.commit().await.unwrap();
            row.unwrap()
        }
    };

    assert_eq!(by_token.get::<Uuid, _>("id"), id);
    assert_eq!(by_token.get::<Uuid, _>("tenant_id"), hotel.tenant);
    assert_eq!(by_token.get::<String, _>("tenant_name"), "Palm Hotel");
    assert_eq!(by_token.get::<String, _>("inviter_name"), "Inviter");
    assert_eq!(by_token.get::<String, _>("status"), "pending");
    assert!(unknown.is_none());
    let own = by_id(Some(hotel.tenant)).await.expect("found in its own tenant");
    assert_eq!(own.get::<Uuid, _>("tenant_id"), hotel.tenant);
    assert_eq!(own.get::<String, _>("email"), "kamala@example.com");
    assert_eq!(own.get::<String, _>("tenant_name"), "Palm Hotel");
    assert_eq!(own.get::<Vec<u8>, _>("token_hash"), hash);
    assert!(by_id(Some(other.tenant)).await.is_none(), "found from another tenant");
    assert!(by_id(None).await.is_none(), "found with no tenant");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_invitation_is_accepted_under_row_level_security(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    let other = hotel(&owner, "Other Hotel").await;
    let hotel = hotel(&owner, "Palm Hotel").await;
    let (fresh, _) = invite(&owner, &hotel, "a@example.com", "7 days").await;
    let accept = |tenant: Uuid| {
        let pool = pool.clone();
        async move {
            let mut tx = begin(&pool, Scope::tenant(TenantId(tenant))).await.unwrap();
            let done = sqlx::query(
                "update invitation set status = 'accepted', accepted_at = now(), accepted_user_id = $2
                 where id = $1 and status = 'pending' and expires_at > now()",
            )
            .bind(fresh)
            .bind(hotel.inviter)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
            tx.commit().await.unwrap();
            done
        }
    };

    assert_eq!(accept(other.tenant).await, 0, "another tenant accepted it");
    assert_eq!(accept(hotel.tenant).await, 1);
    let gone: (bool, bool) = sqlx::query_as(
        "select exists (select 1 from pg_proc p join pg_namespace n on n.oid = p.pronamespace
                        where n.nspname = 'app' and p.proname = 'accept_invitation_row'),
                exists (select 1 from pg_policy where polname = 'invitation_owner_accept')",
    )
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(gone, (false, false), "the accepting definer and its policy are dropped");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_token_definers_pin_their_search_path(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();

    let pinned: Vec<(String, bool, bool)> = sqlx::query_as(
        "select p.proname, p.prosecdef, 'search_path=pg_catalog, public, pg_temp' = any (p.proconfig)
         from pg_proc p join pg_namespace n on n.oid = p.pronamespace
         where n.nspname = 'app' and p.proname in ('invitation_by_token', 'invitation_by_id')
         order by p.proname",
    )
    .fetch_all(&owner)
    .await
    .unwrap();

    let names: Vec<&str> = pinned.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(names, ["invitation_by_id", "invitation_by_token"]);
    assert!(pinned.iter().all(|(_, definer, path)| *definer && *path), "{pinned:?}");
}
