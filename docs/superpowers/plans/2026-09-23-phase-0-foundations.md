# Phase 0: Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A running multi-tenant base: a user signs up, creates properties, switches between tenants, and sees changes live. Tenant isolation is enforced by the database and proven by tests.

**Architecture:** A Cargo workspace with one axum service (`core-api`) over domain modules (`identity`, `property`) and a shared `db` crate. Postgres row-level security isolates tenants: every transaction is opened through `db::begin`, which sets the tenant transaction-locally. REST carries commands (with idempotency keys and CSRF protection), GraphQL carries reads, and server-sent events fed by Postgres `LISTEN/NOTIFY` tell the SvelteKit SPA which cached queries to refetch.

**Tech Stack:** Rust 1.97 (edition 2024), axum 0.8, sqlx 0.9 (Postgres), async-graphql 7.2, utoipa 6, argon2 0.6, mimalloc; PostgreSQL 17; SvelteKit 2 / Svelte 5 (runes) with Bun 1.3, TanStack Query 6, openapi-fetch, GraphQL Codegen.

**Spec:** [docs/ARCHITECTURE.md](../../ARCHITECTURE.md) and [docs/ROADMAP.md](../../ROADMAP.md) (Phase 0). Shared conventions: [docs/design/](../../design/).

**Verified:** every task in this plan was executed in order on a clean repository before the plan was written. The failing step failed, the passing step passed (40 Rust tests, 6 web tests), and `rustfmt`, `clippy -D warnings`, `svelte-check`, ESLint, Prettier and the production build were clean after each task. Task 15 (container image, CI) could not be run in the planning environment; its steps say how to check it.

## Global Constraints

- Rust toolchain `1.97` (pinned in `rust-toolchain.toml`), edition 2024, `unsafe_code = "forbid"`, `clippy::all = deny`, rustfmt `max_width = 120`.
- Crate versions exactly as in the root `Cargo.toml` `[workspace.dependencies]` (Task 1). Crates depend on them with `name.workspace = true`.
- PostgreSQL 17. Local: `docker compose` (or `podman compose`). Tests need a superuser URL in `DATABASE_URL`, because `#[sqlx::test]` creates a throwaway database per test: `export TEST_DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`.
- **All data access goes through `db::begin(pool, scope)`.** Never `pool.begin()` directly for tenant data, and never plain `SET` (only transaction-local `set_config(.., true)`).
- **Every table with rows belonging to a tenant has a `tenant_id` column, forced row-level security, and a case in `crates/db/tests/isolation.rs`.** The schema guard test (Task 1) fails if RLS is missing. Tables that are not tenant-owned must not use the column name `tenant_id`.
- The API connects as `goodfolk_api` (member of `goodfolk_app`, never an owner, never `BYPASSRLS`). Migrations run as the owner through `core-api migrate`.
- SQL is written with `sqlx::query*` functions (checked at run time), and every query is exercised by an integration test against real Postgres.
- IDs are UUIDv7 generated in Rust (`Uuid::now_v7()`). Timestamps are `timestamptz`.
- Errors leave the API as RFC 9457 `application/problem+json`. State-changing requests require the `x-goodfolk-csrf` header; create commands require `Idempotency-Key`.
- Frontend: Bun 1.3+, `sv@0.17.1` scaffold, dependency versions pinned exactly as in Task 13. Generated API types (`src/lib/api/{openapi.json,openapi.d.ts,schema.graphql,gql/}`) are committed and must match the backend.
- Commit messages describe only the change (no tool or AI attribution).

## File Structure

```
Cargo.toml                    workspace, shared dependency versions, lints, release profile
rust-toolchain.toml  rustfmt.toml  deny.toml
compose.yaml                  local Postgres 17
deploy/dev/postgres-init.sql  creates goodfolk_app / goodfolk_api roles
migrations/0001_foundation.sql
crates/db/                    pool, MIGRATOR, Scope + begin (RLS context), Event + notify, testing::app_pool
crates/core-api/              axum service
  src/config.rs               env config (Cloud Run PORT, pooled + direct DB URLs)
  src/error.rs                ApiError → problem+json, validate()
  src/state.rs                AppState (pool, GraphQL schema, event broadcast)
  src/auth.rs                 session cookie, Authenticated / TenantContext extractors
  src/csrf.rs                 x-goodfolk-csrf requirement
  src/idempotency.rs          Idempotency-Key middleware
  src/graphql.rs              read-only schema
  src/events.rs               LISTEN → broadcast → SSE
  src/openapi.rs              OpenAPI document
  src/routes/{mod,health,auth,properties}.rs
  src/main.rs                 `serve` / `migrate`
  src/bin/export-schemas.rs   writes openapi.json + schema.graphql for the frontend
modules/identity/             passwords, roles/grants, sign-up, login, sessions, profile
modules/property/             create/list properties
web/pms/                      SvelteKit SPA (adapter-static, SPA fallback)
  src/lib/api/                REST client, GraphQL client, problem errors, generated types
  src/lib/{events,session,properties}.ts
  src/routes/(auth)/…         login, signup
  src/routes/(app)/…          shell, property list, new property, property page
Dockerfile  .dockerignore  .github/workflows/ci.yml  README.md
```

## Deferred to later phases

These were in the first draft of Phase 0. They were moved because nothing in Phase 0 uses them yet; the roadmap lists each under the phase that first needs it.

| Item | Moved to | Why |
|---|---|---|
| Pub/Sub adapter, transactional outbox, `/proto` crate + tonic template, MinIO in compose | Phase 6 (first internal service: `media-svc`) | No service consumes events or gRPC until then. Live UI updates use Postgres `LISTEN/NOTIFY`, which works across instances with no extra infrastructure |
| `If-Match` optimistic concurrency | Phase 1 (first update endpoints) | Phase 0 has no updates |
| Login throttling / rate limits | Phase 1 | Needs a decision on the per-IP source behind Cloud Run and Cloudflare |
| Playwright end-to-end tests | Phase 1 | The first multi-screen workflows (rooms, blocks) are where they pay off |
| Persisted GraphQL queries (allowlist) | Phase 4 (tape chart) | Depth and complexity limits cover Phase 0's single query |
| Real user invitations (non-owner staff) | Phase 8 (users & roles UI) | Roles and grants exist now; inviting users comes with the settings UI |

## Tasks

### Task 1: Workspace, local Postgres and the foundation schema

Create the Cargo workspace, a local Postgres with the two database roles, and the first migration: tenants, users, memberships, properties, role grants, sessions, audit log and idempotency keys, all tenant-owned tables under forced row-level security. A guard test fails if any future table with a `tenant_id` column is left unprotected.

**Files:**
- Create: `.gitignore`
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `rustfmt.toml`
- Create: `deny.toml`
- Create: `compose.yaml`
- Create: `deploy/dev/postgres-init.sql`
- Create: `crates/db/tests/schema.rs`
- Create: `crates/db/Cargo.toml`
- Create: `crates/db/src/lib.rs`
- Create: `migrations/0001_foundation.sql`

**Interfaces:**
- Produces: `db::MIGRATOR` (`sqlx::migrate::Migrator`), `db::connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error>`.
- Produces: SQL functions `app.current_tenant() -> uuid` and `app.current_user_id() -> uuid`, read from the transaction-local settings `app.tenant_id` / `app.user_id`.
- Produces: roles `goodfolk_app` (NOLOGIN; RLS applies) and `goodfolk_api` (LOGIN, member of `goodfolk_app`; the API connects as this).

- [ ] **Step 1: Start Postgres**

Create the files below first (`compose.yaml` and `deploy/dev/postgres-init.sql`), then:

```sh
docker compose up -d postgres
export TEST_DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
```

Expected: `docker compose ps` shows postgres `healthy`.

- [ ] **Step 2: Create the workspace files**

`.gitignore`:

```gitignore
/target
.env
.env.*
```

`Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
edition = "2024"
rust-version = "1.97"
publish = false

[workspace.dependencies]
db = { path = "crates/db" }
identity = { path = "modules/identity" }
property = { path = "modules/property" }

anyhow = "1"
argon2 = "0.6"
async-graphql = { version = "7.2", default-features = false, features = ["uuid", "time"] }
async-graphql-axum = "7.2"
axum = { version = "0.8", features = ["macros"] }
axum-extra = { version = "0.12", features = ["cookie"] }
base64 = "0.23"
futures = "0.3"
garde = { version = "0.23", features = ["derive", "email"] }
getrandom = "0.3"
http-body-util = "0.1"
mimalloc = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.11"
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "uuid", "time", "json", "migrate", "macros"] }
thiserror = "2"
time = { version = "0.3", features = ["serde", "serde-well-known"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync"] }
tokio-stream = { version = "0.1", features = ["sync"] }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.7", features = ["trace", "compression-br", "compression-zstd", "request-id", "timeout", "util"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
utoipa = { version = "6", features = ["uuid", "time"] }
uuid = { version = "1", features = ["v7", "serde"] }

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }

[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
opt-level = 3
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.97"
components = ["rustfmt", "clippy"]
```

`rustfmt.toml`:

```toml
max_width = 120
use_small_heuristics = "Max"
```

`deny.toml`:

```toml
[graph]
all-features = true

[advisories]
# Fail on any RustSec advisory (vulnerability, unmaintained, unsound, yanked).
version = 2
yanked = "deny"

[licenses]
version = 2
confidence-threshold = 0.9
allow = [
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "CDLA-Permissive-2.0",
  "ISC",
  "MIT",
  "MPL-2.0",
  "Unicode-3.0",
  "Zlib",
]

[licenses.private]
ignore = true

[bans]
multiple-versions = "warn"
wildcards = "deny"
allow-wildcard-paths = true

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

`compose.yaml`:

```yaml
# Local development services. Production uses Neon (see docs/ARCHITECTURE.md §3.4).
services:
  postgres:
    image: postgres:17
    environment:
      # Superuser for migrations and for tests (which create throwaway databases).
      POSTGRES_USER: goodfolk_owner
      POSTGRES_PASSWORD: goodfolk_owner_dev
      POSTGRES_DB: goodfolk
    ports:
      - "5432:5432"
    volumes:
      - ./deploy/dev/postgres-init.sql:/docker-entrypoint-initdb.d/01-roles.sql:ro
      - postgres-data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD", "pg_isready", "-U", "goodfolk_owner"]
      interval: 2s
      retries: 30

volumes:
  postgres-data:
```

`deploy/dev/postgres-init.sql`:

```sql
-- Runs once when the dev/CI Postgres container is first created.
-- goodfolk_app: NOLOGIN group role that owns no tables; RLS applies to it.
-- goodfolk_api: the login role the API connects as.
create role goodfolk_app nologin;
create role goodfolk_api login password 'goodfolk_api_dev' in role goodfolk_app;
```

- [ ] **Step 3: Write the failing test**

Create `crates/db/tests/schema.rs`:

```rust
//! Guards that hold for every migration, including future ones.

use sqlx::PgPool;

/// A table with a `tenant_id` column must have row-level security enabled and forced,
/// or one tenant could read another's rows.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_tenant_table_has_forced_row_level_security(pool: PgPool) {
    let unprotected: Vec<String> = sqlx::query_scalar(
        "select c.relname::text
         from pg_class c
         join pg_namespace n on n.oid = c.relnamespace
         join pg_attribute a on a.attrelid = c.oid and a.attname = 'tenant_id' and not a.attisdropped
         where n.nspname = 'public' and c.relkind in ('r', 'p')
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
```

- [ ] **Step 4: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test schema
```

Expected: FAIL: `cargo` cannot load workspace member `crates/db` (its manifest does not exist yet).

- [ ] **Step 5: Implement**

Create `crates/db/Cargo.toml`:

```toml
[package]
name = "db"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
sqlx.workspace = true

[dev-dependencies]
tokio.workspace = true

[lints]
workspace = true
```

Create `crates/db/src/lib.rs`:

```rust
//! Database access shared by every module: pool setup, migrations, tenant-scoped
//! transactions and change notifications.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(max_connections).acquire_timeout(Duration::from_secs(3)).connect(url).await
}
```

Create `migrations/0001_foundation.sql`:

```sql
create extension if not exists citext;
create schema if not exists app;

-- The app role is normally created by the environment (deploy/dev/postgres-init.sql,
-- or by hand on Neon). Creating it here keeps a bare database usable.
do $$
begin
  create role goodfolk_app nologin;
exception when duplicate_object then null;
end $$;

grant usage on schema public, app to goodfolk_app;

-- Tenant context is set per transaction with set_config(..., true).
-- STABLE functions wrapped in (select ...) inside policies are evaluated once per query.
create function app.current_tenant() returns uuid language sql stable
as $$ select nullif(current_setting('app.tenant_id', true), '')::uuid $$;

create function app.current_user_id() returns uuid language sql stable
as $$ select nullif(current_setting('app.user_id', true), '')::uuid $$;

create table tenant (
  id uuid primary key,
  name text not null check (length(name) between 1 and 200),
  created_at timestamptz not null default now()
);

create table app_user (
  id uuid primary key,
  email citext not null unique,
  password_hash text not null,
  display_name text not null check (length(display_name) between 1 and 200),
  created_at timestamptz not null default now()
);

create table membership (
  tenant_id uuid not null references tenant(id) on delete cascade,
  user_id uuid not null references app_user(id) on delete cascade,
  created_at timestamptz not null default now(),
  primary key (tenant_id, user_id)
);
create index membership_user_idx on membership(user_id);

create table property (
  id uuid primary key,
  tenant_id uuid not null references tenant(id) on delete cascade,
  code text not null check (code ~ '^[A-Z0-9]{2,10}$'),
  name text not null check (length(name) between 1 and 200),
  timezone text not null,
  base_currency char(3) not null check (base_currency ~ '^[A-Z]{3}$'),
  version integer not null default 1,
  created_at timestamptz not null default now(),
  unique (tenant_id, code)
);

create table role_grant (
  id uuid primary key,
  tenant_id uuid not null,
  user_id uuid not null,
  property_id uuid references property(id) on delete cascade,
  role text not null check (role in ('owner', 'manager', 'front_desk', 'housekeeping', 'accountant')),
  foreign key (tenant_id, user_id) references membership(tenant_id, user_id) on delete cascade,
  unique nulls not distinct (tenant_id, user_id, property_id, role)
);
create index role_grant_user_idx on role_grant(tenant_id, user_id);

-- Looked up by token before any tenant is known, so no RLS. active_tenant_id is the tenant the
-- user is working in, not an owner: only tables whose rows belong to a tenant use `tenant_id`.
create table session (
  id uuid primary key,
  token_hash bytea not null unique,
  user_id uuid not null references app_user(id) on delete cascade,
  active_tenant_id uuid references tenant(id) on delete set null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null
);
create index session_user_idx on session(user_id);

create table audit_log (
  id uuid primary key,
  tenant_id uuid not null references tenant(id) on delete cascade,
  actor_user_id uuid references app_user(id) on delete set null,
  action text not null,
  entity text not null,
  entity_id uuid,
  data jsonb not null default '{}',
  at timestamptz not null default now()
);
create index audit_log_tenant_at_idx on audit_log(tenant_id, at desc);

create table idempotency_key (
  tenant_id uuid not null references tenant(id) on delete cascade,
  key text not null check (length(key) between 8 and 200),
  request_hash bytea not null,
  status_code smallint,
  response_body bytea,
  created_at timestamptz not null default now(),
  primary key (tenant_id, key)
);

-- Row-level security. app_user and session are global identity tables (no tenant column).
alter table tenant enable row level security;
alter table tenant force row level security;
create policy tenant_read on tenant for select using (
  id = (select app.current_tenant())
  or id in (select tenant_id from membership where user_id = (select app.current_user_id()))
);
create policy tenant_write on tenant for all
  using (id = (select app.current_tenant()))
  with check (id = (select app.current_tenant()));

alter table membership enable row level security;
alter table membership force row level security;
create policy membership_read on membership for select using (
  tenant_id = (select app.current_tenant()) or user_id = (select app.current_user_id())
);
create policy membership_write on membership for all
  using (tenant_id = (select app.current_tenant()))
  with check (tenant_id = (select app.current_tenant()));

do $$
declare t text;
begin
  foreach t in array array['property', 'role_grant', 'audit_log', 'idempotency_key'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;

grant select, insert, update, delete on all tables in schema public to goodfolk_app;
revoke all on _sqlx_migrations from goodfolk_app;
revoke update, delete on audit_log from goodfolk_app;
grant execute on all functions in schema app to goodfolk_app;
alter default privileges in schema public grant select, insert, update, delete on tables to goodfolk_app;
```

- [ ] **Step 6: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test schema
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 2 tests (`every_tenant_table_has_forced_row_level_security`, `the_app_role_cannot_bypass_row_level_security`). `fmt` and `clippy` print nothing.

- [ ] **Step 7: Commit**

```sh
git add -A
git commit -m "chore: workspace, local Postgres and foundation schema with row-level security"
```

### Task 2: Tenant-scoped transactions and the isolation suite

Every database access goes through `db::begin(pool, scope)`, which sets the tenant and user for row-level security *inside the transaction only*, so context can never leak to the next user of a pooled connection. The isolation suite proves a tenant sees and writes only its own rows. **Every tenant-scoped table added in later phases gets a case in this suite.**

**Files:**
- Create: `crates/db/tests/isolation.rs`
- Modify: `crates/db/Cargo.toml`
- Modify: `crates/db/src/lib.rs`
- Create: `crates/db/src/scope.rs`
- Create: `crates/db/src/testing.rs`

**Interfaces:**
- Produces: `db::TenantId(pub Uuid)`, `db::UserId(pub Uuid)` (serde-transparent newtypes), `db::Tx = sqlx::Transaction<'static, Postgres>`.
- Produces: `db::Scope { tenant: Option<TenantId>, user: Option<UserId> }` with `Scope::tenant(t)`, `Scope::user(u)`, `Scope::default()`.
- Produces: `db::begin(pool: &PgPool, scope: Scope) -> Result<Tx, sqlx::Error>`.
- Produces (feature `testing`, dev-dependencies only): `db::testing::app_pool(opts: PgConnectOptions, max_connections: u32) -> PgPool`, a pool that runs `set role goodfolk_app` on connect so tests see RLS exactly as production does.

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/isolation.rs`:

```rust
//! Tenant isolation suite. Every tenant-scoped table added later gets a case here.

use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

async fn seed_tenant(pool: &PgPool, name: &str) -> TenantId {
    let tenant = TenantId(Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("insert into tenant (id, name) values ($1, $2)")
        .bind(tenant.0)
        .bind(name)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency)
         values ($1, $2, 'MAIN', $3, 'Asia/Colombo', 'LKR')",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(format!("{name} Hotel"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    tenant
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_tenant_sees_only_its_own_rows(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;

    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    let tenants: Vec<Uuid> = sqlx::query_scalar("select id from tenant").fetch_all(&mut *tx).await.unwrap();
    let properties: Vec<Uuid> = sqlx::query_scalar("select tenant_id from property").fetch_all(&mut *tx).await.unwrap();

    assert_eq!(tenants, vec![a.0]);
    assert_eq!(properties, vec![a.0]);
    assert!(!properties.contains(&b.0));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn no_scope_sees_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    seed_tenant(&pool, "A").await;

    let mut tx = begin(&pool, Scope::default()).await.unwrap();
    let count: i64 = sqlx::query_scalar("select count(*) from property").fetch_one(&mut *tx).await.unwrap();

    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn writing_into_another_tenant_is_rejected(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;

    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    let result = sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency)
         values ($1, $2, 'X1', 'Intruder', 'Asia/Colombo', 'LKR')",
    )
    .bind(Uuid::now_v7())
    .bind(b.0)
    .execute(&mut *tx)
    .await;

    let err = result.unwrap_err().to_string();
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tenant_context_does_not_leak_between_transactions(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;

    begin(&pool, Scope::tenant(a)).await.unwrap().commit().await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let setting: Option<String> = sqlx::query_scalar("select nullif(current_setting('app.tenant_id', true), '')")
        .fetch_one(&mut *conn)
        .await
        .unwrap();

    assert_eq!(setting, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_user_sees_tenants_they_belong_to_but_cannot_join_others(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    let user = UserId(Uuid::now_v7());
    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, 'u@example.com', 'x', 'U')")
        .bind(user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(a.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin(&pool, Scope::user(user)).await.unwrap();
    let visible: Vec<Uuid> = sqlx::query_scalar("select id from tenant").fetch_all(&mut *tx).await.unwrap();
    let join = sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(b.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await;

    assert_eq!(visible, vec![a.0]);
    assert!(join.is_err());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn audit_log_is_append_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    sqlx::query("insert into audit_log (id, tenant_id, action, entity) values ($1, $2, 'test', 'tenant')")
        .bind(Uuid::now_v7())
        .bind(a.0)
        .execute(&mut *tx)
        .await
        .unwrap();

    let delete = sqlx::query("delete from audit_log").execute(&mut *tx).await;

    assert!(delete.is_err());
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test isolation
```

Expected: FAIL: compile errors, `db::testing`, `db::begin`, `db::Scope` not found.

- [ ] **Step 3: Implement**

Replace the whole file `crates/db/Cargo.toml`:

```toml
[package]
name = "db"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true
sqlx.workspace = true
uuid.workspace = true

[features]
testing = []

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
tokio.workspace = true

[lints]
workspace = true
```

Replace the whole file `crates/db/src/lib.rs`:

```rust
//! Database access shared by every module: pool setup, migrations, tenant-scoped
//! transactions and change notifications.

mod scope;
#[cfg(feature = "testing")]
pub mod testing;

pub use scope::{Scope, TenantId, Tx, UserId, begin};

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(max_connections).acquire_timeout(Duration::from_secs(3)).connect(url).await
}
```

Create `crates/db/src/scope.rs`:

```rust
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub Uuid);

pub type Tx = sqlx::Transaction<'static, sqlx::Postgres>;

/// Who a transaction acts for. Row-level security policies read these values.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scope {
    pub tenant: Option<TenantId>,
    pub user: Option<UserId>,
}

impl Scope {
    pub fn tenant(tenant: TenantId) -> Self {
        Self { tenant: Some(tenant), user: None }
    }

    pub fn user(user: UserId) -> Self {
        Self { tenant: None, user: Some(user) }
    }
}

/// Begins a transaction whose row-level security context is `scope`.
///
/// The settings are transaction-local (`set_config(.., true)`), so they end with the
/// transaction and can never leak to the next user of a pooled connection.
pub async fn begin(pool: &PgPool, scope: Scope) -> Result<Tx, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("select set_config('app.tenant_id', $1, true), set_config('app.user_id', $2, true)")
        .bind(scope.tenant.map(|t| t.0.to_string()).unwrap_or_default())
        .bind(scope.user.map(|u| u.0.to_string()).unwrap_or_default())
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
```

Create `crates/db/src/testing.rs`:

```rust
//! Test helpers. Enabled with the `testing` feature, for dev-dependencies only.

use sqlx::Executor;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

/// A pool acting as `goodfolk_app`, so row-level security applies
/// exactly as in production. (`#[sqlx::test]` connects as a superuser, which bypasses RLS.)
pub async fn app_pool(opts: PgConnectOptions, max_connections: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .after_connect(|conn, _| {
            Box::pin(async move {
                conn.execute("set role goodfolk_app").await?;
                Ok(())
            })
        })
        .connect_with(opts)
        .await
        .expect("connect test pool")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 6 isolation tests + 2 schema tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(db): tenant-scoped transactions and tenant isolation suite"
```

### Task 3: Change notifications

Writes announce what changed with `db::notify(tx, event)`, a Postgres `NOTIFY` that is delivered only if the transaction commits. Every API instance listens and forwards these to browsers (Task 11), which refetch just the named cache keys. Events carry keys, never data.

**Files:**
- Create: `crates/db/tests/events.rs`
- Modify: `crates/db/Cargo.toml`
- Modify: `crates/db/src/lib.rs`
- Create: `crates/db/src/events.rs`

**Interfaces:**
- Produces: `db::CHANNEL: &str = "gf_events"`.
- Produces: `db::Event { tenant_id: TenantId, property_id: Option<Uuid>, keys: Vec<String> }` (serde, `PartialEq`).
- Produces: `db::notify(tx: &mut Tx, event: &Event) -> Result<(), sqlx::Error>`.

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/events.rs`:

```rust
use db::testing::app_pool;
use db::{CHANNEL, Event, Scope, TenantId, begin, notify};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

fn event(tenant: TenantId) -> Event {
    Event { tenant_id: tenant, property_id: None, keys: vec!["properties".into()] }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn events_are_delivered_only_when_the_transaction_commits(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 2).await;
    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen(CHANNEL).await.unwrap();
    let tenant = TenantId(Uuid::now_v7());

    let mut rolled_back = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    notify(&mut rolled_back, &Event { keys: vec!["discarded".into()], ..event(tenant) }).await.unwrap();
    rolled_back.rollback().await.unwrap();
    let mut committed = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    notify(&mut committed, &event(tenant)).await.unwrap();
    committed.commit().await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await.unwrap().unwrap();
    let decoded: Event = serde_json::from_str(received.payload()).unwrap();
    assert_eq!(decoded, event(tenant));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test events
```

Expected: FAIL: compile errors, `db::notify`, `db::Event`, `db::CHANNEL` not found.

- [ ] **Step 3: Implement**

Replace the whole file `crates/db/Cargo.toml`:

```toml
[package]
name = "db"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
uuid.workspace = true

[features]
testing = []

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
tokio.workspace = true

[lints]
workspace = true
```

Replace the whole file `crates/db/src/lib.rs`:

```rust
//! Database access shared by every module: pool setup, migrations, tenant-scoped
//! transactions and change notifications.

mod events;
mod scope;
#[cfg(feature = "testing")]
pub mod testing;

pub use events::{CHANNEL, Event, notify};
pub use scope::{Scope, TenantId, Tx, UserId, begin};

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(max_connections).acquire_timeout(Duration::from_secs(3)).connect(url).await
}
```

Create `crates/db/src/events.rs`:

```rust
use crate::{TenantId, Tx};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Postgres NOTIFY channel carrying cache-invalidation events to every API instance.
pub const CHANNEL: &str = "gf_events";

/// Tells clients which cached data changed. Carries keys, never the data itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub tenant_id: TenantId,
    pub property_id: Option<Uuid>,
    pub keys: Vec<String>,
}

/// Queues `event` for delivery. Postgres delivers it only if `tx` commits.
pub async fn notify(tx: &mut Tx, event: &Event) -> Result<(), sqlx::Error> {
    let payload = serde_json::to_string(event).expect("Event always serializes");
    sqlx::query("select pg_notify($1, $2)").bind(CHANNEL).bind(payload).execute(&mut **tx).await?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: all `db` tests (9). `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(db): transactional change notifications"
```

### Task 4: Password hashing and roles

The `identity` module starts with two pure pieces: Argon2id password hashing and role-based permissions. A grant is a role held tenant-wide (`property_id: None`) or for one property. Permissions are added to the `Permission` enum as each phase introduces new actions.

**Files:**
- Create: `modules/identity/tests/password.rs`
- Create: `modules/identity/tests/rbac.rs`
- Modify: `Cargo.toml`
- Create: `modules/identity/Cargo.toml`
- Create: `modules/identity/src/lib.rs`
- Create: `modules/identity/src/password.rs`
- Create: `modules/identity/src/rbac.rs`

**Interfaces:**
- Produces: `identity::hash_password(&str) -> String`, `identity::verify_password(&str, &str) -> bool` (both CPU-heavy: call inside `tokio::task::spawn_blocking`).
- Produces: `identity::Role { Owner, Manager, FrontDesk, Housekeeping, Accountant }` with `as_str()` / `parse()` matching the `role_grant.role` check constraint.
- Produces: `identity::Permission { PropertiesView, PropertiesCreate }`, `identity::Grant { property_id: Option<Uuid>, role: Role }`.
- Produces: `identity::allows(&[Grant], Permission, property: Option<Uuid>) -> bool`, `identity::load_grants(&mut Tx, UserId) -> Result<Vec<Grant>, sqlx::Error>`.

- [ ] **Step 1: Write the failing tests**

Create `modules/identity/tests/password.rs`:

```rust
use identity::{hash_password, verify_password};

#[test]
fn a_hash_verifies_only_its_own_password() {
    let hash = hash_password("correct horse battery staple");

    assert!(hash.starts_with("$argon2id$"));
    assert!(verify_password("correct horse battery staple", &hash));
    assert!(!verify_password("wrong", &hash));
}

#[test]
fn a_malformed_hash_never_verifies() {
    assert!(!verify_password("anything", "not-a-hash"));
}
```

Create `modules/identity/tests/rbac.rs`:

```rust
use identity::{Grant, Permission, Role, allows};
use uuid::Uuid;

const HOTEL: Uuid = Uuid::from_u128(1);
const OTHER_HOTEL: Uuid = Uuid::from_u128(2);

#[test]
fn an_owner_may_do_anything_anywhere() {
    let grants = [Grant { property_id: None, role: Role::Owner }];

    assert!(allows(&grants, Permission::PropertiesCreate, None));
    assert!(allows(&grants, Permission::PropertiesView, Some(HOTEL)));
}

#[test]
fn a_property_grant_applies_only_to_that_property() {
    let grants = [Grant { property_id: Some(HOTEL), role: Role::FrontDesk }];

    assert!(allows(&grants, Permission::PropertiesView, Some(HOTEL)));
    assert!(!allows(&grants, Permission::PropertiesView, Some(OTHER_HOTEL)));
    assert!(!allows(&grants, Permission::PropertiesView, None));
}

#[test]
fn only_owners_create_properties() {
    let manager = [Grant { property_id: None, role: Role::Manager }];

    assert!(!allows(&manager, Permission::PropertiesCreate, None));
}

#[test]
fn roles_round_trip_through_their_database_names() {
    for role in [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::Accountant] {
        assert_eq!(Role::parse(role.as_str()), Some(role));
    }
    assert_eq!(Role::parse("root"), None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
cargo test -p identity --test password --test rbac
```

Expected: FAIL: `cargo` cannot find package `identity`.

- [ ] **Step 3: Implement**

Replace the whole file `Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["crates/*", "modules/*"]

[workspace.package]
edition = "2024"
rust-version = "1.97"
publish = false

[workspace.dependencies]
db = { path = "crates/db" }
identity = { path = "modules/identity" }
property = { path = "modules/property" }

anyhow = "1"
argon2 = "0.6"
async-graphql = { version = "7.2", default-features = false, features = ["uuid", "time"] }
async-graphql-axum = "7.2"
axum = { version = "0.8", features = ["macros"] }
axum-extra = { version = "0.12", features = ["cookie"] }
base64 = "0.23"
futures = "0.3"
garde = { version = "0.23", features = ["derive", "email"] }
getrandom = "0.3"
http-body-util = "0.1"
mimalloc = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.11"
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "uuid", "time", "json", "migrate", "macros"] }
thiserror = "2"
time = { version = "0.3", features = ["serde", "serde-well-known"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync"] }
tokio-stream = { version = "0.1", features = ["sync"] }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.7", features = ["trace", "compression-br", "compression-zstd", "request-id", "timeout", "util"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
utoipa = { version = "6", features = ["uuid", "time"] }
uuid = { version = "1", features = ["v7", "serde"] }

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }

[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
opt-level = 3
```

Create `modules/identity/Cargo.toml`:

```toml
[package]
name = "identity"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
argon2.workspace = true
db.workspace = true
serde.workspace = true
sqlx.workspace = true
utoipa.workspace = true
uuid.workspace = true

[lints]
workspace = true
```

Create `modules/identity/src/lib.rs`:

```rust
//! Users, tenants, sessions and role-based permissions.

mod password;
mod rbac;

pub use password::{hash_password, verify_password};
pub use rbac::{Grant, Permission, Role, allows, load_grants};
```

Create `modules/identity/src/password.rs`:

```rust
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

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
```

Create `modules/identity/src/rbac.rs`:

```rust
use db::{Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Manager,
    FrontDesk,
    Housekeeping,
    Accountant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Manager => "manager",
            Role::FrontDesk => "front_desk",
            Role::Housekeeping => "housekeeping",
            Role::Accountant => "accountant",
        }
    }

    pub fn parse(value: &str) -> Option<Role> {
        [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::Accountant]
            .into_iter()
            .find(|role| role.as_str() == value)
    }

    fn permits(self, permission: Permission) -> bool {
        use Permission::*;
        match self {
            Role::Owner => true,
            Role::Manager | Role::FrontDesk | Role::Housekeeping | Role::Accountant => {
                matches!(permission, PropertiesView)
            }
        }
    }
}

/// Permissions are added here as each phase introduces new actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    PropertiesView,
    PropertiesCreate,
}

/// A role held tenant-wide (`property_id: None`) or for one property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Grant {
    pub property_id: Option<Uuid>,
    pub role: Role,
}

/// `property: None` asks for a tenant-wide permission, which only tenant-wide grants give.
pub fn allows(grants: &[Grant], permission: Permission, property: Option<Uuid>) -> bool {
    grants.iter().any(|grant| {
        let in_scope = grant.property_id.is_none() || grant.property_id == property;
        in_scope && grant.role.permits(permission)
    })
}

/// Loads `user`'s grants in the tenant that `tx` is scoped to.
pub async fn load_grants(tx: &mut Tx, user: UserId) -> Result<Vec<Grant>, sqlx::Error> {
    let rows: Vec<(Option<Uuid>, String)> =
        sqlx::query_as("select property_id, role from role_grant where user_id = $1")
            .bind(user.0)
            .fetch_all(&mut **tx)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(property_id, role)| Role::parse(&role).map(|role| Grant { property_id, role }))
        .collect())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo test -p identity --test password --test rbac
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 6 tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(identity): argon2id password hashing and role grants"
```

### Task 5: Accounts and sessions

Sign-up creates a user, their tenant, a membership and an owner grant in one transaction. Sessions are opaque random tokens; only their SHA-256 is stored. Login verifies against a dummy hash when the email is unknown, so timing does not reveal which emails exist. Emails are `citext` and must be bound as `$1::citext`, or Postgres compares case-sensitively.

**Files:**
- Create: `modules/identity/tests/accounts.rs`
- Modify: `modules/identity/Cargo.toml`
- Modify: `modules/identity/src/lib.rs`
- Create: `modules/identity/src/account.rs`
- Create: `modules/identity/src/session.rs`

**Interfaces:**
- Consumes: `db::begin`, `db::Scope`, `identity::hash_password`, `identity::load_grants`.
- Produces: `identity::SignupInput { email, password, display_name, tenant_name }` (all `String`), `identity::SignupError { EmailTaken, Database(sqlx::Error) }`.
- Produces: `identity::signup(&PgPool, SignupInput) -> Result<(UserId, TenantId), SignupError>`, `identity::authenticate(&PgPool, email, password) -> Result<Option<UserId>, sqlx::Error>`, `identity::default_tenant(&PgPool, UserId) -> Result<Option<TenantId>, sqlx::Error>`.
- Produces: `identity::SessionInfo { id: Uuid, user: UserId, tenant: Option<TenantId> }`, `create_session(&PgPool, UserId, Option<TenantId>) -> Result<(String, OffsetDateTime), _>`, `resolve_session(&PgPool, &str) -> Result<Option<SessionInfo>, _>`, `delete_session(&PgPool, &str)`, `switch_tenant(&PgPool, &SessionInfo, TenantId) -> Result<bool, _>`, `SESSION_TTL` (14 h).
- Produces: `identity::Profile { user_id, email, display_name, tenants: Vec<TenantSummary>, current_tenant: Option<Uuid>, grants: Vec<Grant> }`, `identity::TenantSummary { id, name }`, `load_profile(&PgPool, UserId, Option<TenantId>) -> Result<Profile, _>`.

- [ ] **Step 1: Write the failing test**

Create `modules/identity/tests/accounts.rs`:

```rust
use db::testing::app_pool;
use identity::{
    Permission, SignupError, SignupInput, allows, authenticate, create_session, default_tenant, delete_session,
    load_profile, resolve_session, signup, switch_tenant,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

fn input(email: &str, tenant: &str) -> SignupInput {
    SignupInput {
        email: email.into(),
        password: "a long enough password".into(),
        display_name: "Nimal".into(),
        tenant_name: tenant.into(),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn signup_creates_an_owner_of_a_new_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;

    let (user, tenant) = signup(&pool, input("owner@example.com", "Lagoon Hotels")).await.unwrap();
    let profile = load_profile(&pool, user, Some(tenant)).await.unwrap();

    assert_eq!(profile.email, "owner@example.com");
    assert_eq!(profile.tenants.len(), 1);
    assert_eq!(profile.tenants[0].name, "Lagoon Hotels");
    assert!(allows(&profile.grants, Permission::PropertiesCreate, None));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn emails_are_unique_ignoring_case(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    signup(&pool, input("owner@example.com", "A")).await.unwrap();

    let second = signup(&pool, input("OWNER@example.com", "B")).await;

    assert!(matches!(second, Err(SignupError::EmailTaken)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn authenticate_accepts_only_the_right_password(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (user, _) = signup(&pool, input("owner@example.com", "A")).await.unwrap();

    assert_eq!(authenticate(&pool, "Owner@Example.com", "a long enough password").await.unwrap(), Some(user));
    assert_eq!(authenticate(&pool, "owner@example.com", "wrong password").await.unwrap(), None);
    assert_eq!(authenticate(&pool, "nobody@example.com", "a long enough password").await.unwrap(), None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_session_resolves_until_deleted(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (user, tenant) = signup(&pool, input("owner@example.com", "A")).await.unwrap();
    assert_eq!(default_tenant(&pool, user).await.unwrap(), Some(tenant));

    let (token, _) = create_session(&pool, user, Some(tenant)).await.unwrap();
    let session = resolve_session(&pool, &token).await.unwrap().unwrap();
    delete_session(&pool, &token).await.unwrap();

    assert_eq!(session.user, user);
    assert_eq!(session.tenant, Some(tenant));
    assert_eq!(resolve_session(&pool, &token).await.unwrap(), None);
    assert_eq!(resolve_session(&pool, "made-up-token").await.unwrap(), None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_session_can_switch_only_to_tenants_the_user_belongs_to(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (user, tenant) = signup(&pool, input("owner@example.com", "A")).await.unwrap();
    let (_, stranger_tenant) = signup(&pool, input("other@example.com", "B")).await.unwrap();
    let (token, _) = create_session(&pool, user, None).await.unwrap();
    let session = resolve_session(&pool, &token).await.unwrap().unwrap();

    assert!(!switch_tenant(&pool, &session, stranger_tenant).await.unwrap());
    assert!(switch_tenant(&pool, &session, tenant).await.unwrap());
    assert_eq!(resolve_session(&pool, &token).await.unwrap().unwrap().tenant, Some(tenant));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p identity --test accounts
```

Expected: FAIL: compile errors (unresolved imports such as `db::testing` and `identity::signup`).

- [ ] **Step 3: Implement**

Replace the whole file `modules/identity/Cargo.toml`:

```toml
[package]
name = "identity"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
argon2.workspace = true
base64.workspace = true
db.workspace = true
getrandom.workspace = true
serde.workspace = true
sha2.workspace = true
sqlx.workspace = true
thiserror.workspace = true
time.workspace = true
tokio.workspace = true
utoipa.workspace = true
uuid.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }

[lints]
workspace = true
```

Replace the whole file `modules/identity/src/lib.rs`:

```rust
//! Users, tenants, sessions and role-based permissions.

mod account;
mod password;
mod rbac;
mod session;

pub use account::{
    Profile, SignupError, SignupInput, TenantSummary, authenticate, default_tenant, load_profile, signup,
};
pub use password::{hash_password, verify_password};
pub use rbac::{Grant, Permission, Role, allows, load_grants};
pub use session::{SESSION_TTL, SessionInfo, create_session, delete_session, resolve_session, switch_tenant};
```

Create `modules/identity/src/account.rs`:

```rust
use crate::rbac::{Grant, Role, load_grants};
use crate::{hash_password, verify_password};
use db::{Scope, TenantId, UserId};
use serde::Serialize;
use sqlx::PgPool;
use std::sync::LazyLock;
use uuid::Uuid;

pub struct SignupInput {
    pub email: String,
    pub password: String,
    pub display_name: String,
    pub tenant_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SignupError {
    #[error("an account with this email already exists")]
    EmailTaken,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Creates a user, their tenant, a membership and an owner grant in one transaction.
pub async fn signup(pool: &PgPool, input: SignupInput) -> Result<(UserId, TenantId), SignupError> {
    let user = UserId(Uuid::now_v7());
    let tenant = TenantId(Uuid::now_v7());
    let password = input.password;
    let hash =
        tokio::task::spawn_blocking(move || hash_password(&password)).await.expect("hashing task does not panic");

    let mut tx = db::begin(pool, Scope { tenant: Some(tenant), user: Some(user) }).await?;
    let inserted = sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, $3, $4)")
        .bind(user.0)
        .bind(&input.email)
        .bind(&hash)
        .bind(&input.display_name)
        .execute(&mut *tx)
        .await;
    if let Err(err) = inserted {
        let taken = err
            .as_database_error()
            .and_then(|db_err| db_err.constraint())
            .is_some_and(|constraint| constraint == "app_user_email_key");
        return Err(if taken { SignupError::EmailTaken } else { err.into() });
    }
    sqlx::query("insert into tenant (id, name) values ($1, $2)")
        .bind(tenant.0)
        .bind(&input.tenant_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(tenant.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, null, $4)")
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(user.0)
        .bind(Role::Owner.as_str())
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id) values ($1, $2, $3, 'tenant.created', 'tenant', $2)")
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((user, tenant))
}

/// Verified against when an email is unknown, so response time does not reveal which emails exist.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| hash_password("dummy password for timing"));

/// Returns the user if the email and password match.
pub async fn authenticate(pool: &PgPool, email: &str, password: &str) -> Result<Option<UserId>, sqlx::Error> {
    let row: Option<(Uuid, String)> = sqlx::query_as("select id, password_hash from app_user where email = $1::citext")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    let password = password.to_owned();
    let (user, hash) = match row {
        Some((id, hash)) => (Some(UserId(id)), hash),
        None => (None, DUMMY_HASH.clone()),
    };
    let valid = tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .expect("verification task does not panic");
    Ok(user.filter(|_| valid))
}

/// Returns a tenant the user belongs to, for new sessions. Tenants are listed oldest first.
pub async fn default_tenant(pool: &PgPool, user: UserId) -> Result<Option<TenantId>, sqlx::Error> {
    let mut tx = db::begin(pool, Scope::user(user)).await?;
    let tenant: Option<Uuid> =
        sqlx::query_scalar("select tenant_id from membership where user_id = $1 order by created_at limit 1")
            .bind(user.0)
            .fetch_optional(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(tenant.map(TenantId))
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TenantSummary {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Profile {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub tenants: Vec<TenantSummary>,
    pub current_tenant: Option<Uuid>,
    pub grants: Vec<Grant>,
}

pub async fn load_profile(pool: &PgPool, user: UserId, current: Option<TenantId>) -> Result<Profile, sqlx::Error> {
    let mut tx = db::begin(pool, Scope { tenant: current, user: Some(user) }).await?;
    let (email, display_name): (String, String) =
        sqlx::query_as("select email::text, display_name from app_user where id = $1")
            .bind(user.0)
            .fetch_one(&mut *tx)
            .await?;
    let tenants: Vec<(Uuid, String)> = sqlx::query_as(
        "select t.id, t.name from tenant t join membership m on m.tenant_id = t.id
         where m.user_id = $1 order by t.name",
    )
    .bind(user.0)
    .fetch_all(&mut *tx)
    .await?;
    let grants = if current.is_some() { load_grants(&mut tx, user).await? } else { Vec::new() };
    tx.commit().await?;
    Ok(Profile {
        user_id: user.0,
        email,
        display_name,
        tenants: tenants.into_iter().map(|(id, name)| TenantSummary { id, name }).collect(),
        current_tenant: current.map(|t| t.0),
        grants,
    })
}
```

Create `modules/identity/src/session.rs`:

```rust
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use db::{TenantId, UserId};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Staff sessions last one working day plus margin.
pub const SESSION_TTL: Duration = Duration::hours(14);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: Uuid,
    pub user: UserId,
    pub tenant: Option<TenantId>,
}

/// Only the SHA-256 of a token is stored, so a database leak does not leak live sessions.
fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Creates a session and returns the opaque token for the cookie.
pub async fn create_session(
    pool: &PgPool,
    user: UserId,
    tenant: Option<TenantId>,
) -> Result<(String, OffsetDateTime), sqlx::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random number generator available");
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let expires_at = OffsetDateTime::now_utc() + SESSION_TTL;
    sqlx::query(
        "insert into session (id, token_hash, user_id, active_tenant_id, expires_at) values ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(token_hash(&token))
    .bind(user.0)
    .bind(tenant.map(|t| t.0))
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok((token, expires_at))
}

/// Returns the live session for `token`, or `None` if it is unknown or expired.
pub async fn resolve_session(pool: &PgPool, token: &str) -> Result<Option<SessionInfo>, sqlx::Error> {
    let row: Option<(Uuid, Uuid, Option<Uuid>)> = sqlx::query_as(
        "select id, user_id, active_tenant_id from session where token_hash = $1 and expires_at > now()",
    )
    .bind(token_hash(token))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, user, tenant)| SessionInfo { id, user: UserId(user), tenant: tenant.map(TenantId) }))
}

pub async fn delete_session(pool: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from session where token_hash = $1").bind(token_hash(token)).execute(pool).await?;
    Ok(())
}

/// Points the session at `tenant`. Returns `false` if the user is not a member of it.
pub async fn switch_tenant(pool: &PgPool, session: &SessionInfo, tenant: TenantId) -> Result<bool, sqlx::Error> {
    let mut tx = db::begin(pool, db::Scope::user(session.user)).await?;
    let member: bool =
        sqlx::query_scalar("select exists (select 1 from membership where tenant_id = $1 and user_id = $2)")
            .bind(tenant.0)
            .bind(session.user.0)
            .fetch_one(&mut *tx)
            .await?;
    if member {
        sqlx::query("update session set active_tenant_id = $1 where id = $2")
            .bind(tenant.0)
            .bind(session.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(member)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p identity
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 11 tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(identity): sign-up, authentication and sessions"
```

### Task 6: Properties module

A property is a hotel within a tenant. Creating one validates the IANA time zone against Postgres' own `pg_timezone_names` (no extra dependency), writes an audit entry and emits a `properties` change event, all in the caller's transaction.

**Files:**
- Create: `modules/property/tests/properties.rs`
- Create: `modules/property/Cargo.toml`
- Create: `modules/property/src/lib.rs`

**Interfaces:**
- Consumes: `db::{Tx, TenantId, UserId, Event, notify}`.
- Produces: `property::Property { id, code, name, timezone, base_currency, version: i32 }` (serde, `sqlx::FromRow`, `utoipa::ToSchema`), `property::NewProperty { code, name, timezone, base_currency }`, `property::PropertyError { CodeTaken, UnknownTimezone, Database }`.
- Produces: `property::create_property(&mut Tx, TenantId, actor: UserId, NewProperty) -> Result<Property, PropertyError>`, `property::list_properties(&mut Tx, only: Option<&[Uuid]>) -> Result<Vec<Property>, sqlx::Error>`, `property::PROPERTIES_KEY = "properties"`.

- [ ] **Step 1: Write the failing test**

Create `modules/property/tests/properties.rs`:

```rust
use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use property::{NewProperty, PropertyError, create_property, list_properties};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

async fn tenant_with_user(pool: &PgPool) -> (TenantId, UserId) {
    let tenant = TenantId(Uuid::now_v7());
    let user = UserId(Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
        .bind(user.0)
        .bind(format!("{}@example.com", user.0))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (tenant, user)
}

fn hotel(code: &str, timezone: &str) -> NewProperty {
    NewProperty {
        code: code.into(),
        name: format!("Hotel {code}"),
        timezone: timezone.into(),
        base_currency: "LKR".into(),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn created_properties_are_listed_by_code(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let galle = create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await.unwrap();
    let kandy = create_property(&mut tx, tenant, user, hotel("KAN", "Asia/Colombo")).await.unwrap();
    let all = list_properties(&mut tx, None).await.unwrap();
    let only_kandy = list_properties(&mut tx, Some(&[kandy.id])).await.unwrap();

    assert_eq!(all, vec![galle, kandy.clone()]);
    assert_eq!(only_kandy, vec![kandy]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn codes_are_unique_within_a_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await.unwrap();

    let again = create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await;

    assert!(matches!(again, Err(PropertyError::CodeTaken)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn unknown_time_zones_are_rejected(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let result = create_property(&mut tx, tenant, user, hotel("GAL", "Mars/Olympus")).await;

    assert!(matches!(result, Err(PropertyError::UnknownTimezone)));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p property
```

Expected: FAIL: `cargo` cannot load the new workspace member (its `Cargo.toml` does not exist yet).

- [ ] **Step 3: Implement**

Create `modules/property/Cargo.toml`:

```toml
[package]
name = "property"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
db.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
thiserror.workspace = true
utoipa.workspace = true
uuid.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
tokio.workspace = true

[lints]
workspace = true
```

Create `modules/property/src/lib.rs`:

```rust
//! Properties (hotels) within a tenant.

use db::{Event, TenantId, Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Property {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
    pub version: i32,
}

pub struct NewProperty {
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PropertyError {
    #[error("a property with this code already exists")]
    CodeTaken,
    #[error("unknown time zone")]
    UnknownTimezone,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key clients refetch when the property list changes.
pub const PROPERTIES_KEY: &str = "properties";

/// `tx` must be scoped to `tenant`.
pub async fn create_property(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    input: NewProperty,
) -> Result<Property, PropertyError> {
    let known_zone: bool = sqlx::query_scalar("select exists (select 1 from pg_timezone_names where name = $1)")
        .bind(&input.timezone)
        .fetch_one(&mut **tx)
        .await?;
    if !known_zone {
        return Err(PropertyError::UnknownTimezone);
    }
    let inserted = sqlx::query_as::<_, Property>(
        "insert into property (id, tenant_id, code, name, timezone, base_currency)
         values ($1, $2, $3, $4, $5, $6)
         returning id, code, name, timezone, base_currency, version",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(&input.code)
    .bind(&input.name)
    .bind(&input.timezone)
    .bind(&input.base_currency)
    .fetch_one(&mut **tx)
    .await;
    let property = match inserted {
        Ok(property) => property,
        Err(err) => {
            let taken = err
                .as_database_error()
                .and_then(|db_err| db_err.constraint())
                .is_some_and(|constraint| constraint == "property_tenant_id_code_key");
            return Err(if taken { PropertyError::CodeTaken } else { err.into() });
        }
    };
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'property.created', 'property', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(property.id)
    .bind(serde_json::json!({ "code": property.code, "name": property.name }))
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: None, keys: vec![PROPERTIES_KEY.into()] }).await?;
    Ok(property)
}

/// Lists properties by code. `only` limits the result to those ids (for property-scoped staff).
pub async fn list_properties(tx: &mut Tx, only: Option<&[Uuid]>) -> Result<Vec<Property>, sqlx::Error> {
    sqlx::query_as(
        "select id, code, name, timezone, base_currency, version from property
         where $1::uuid[] is null or id = any($1)
         order by code",
    )
    .bind(only)
    .fetch_all(&mut **tx)
    .await
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p property
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 3 tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(property): create and list properties"
```

### Task 7: API skeleton: health checks, request IDs, compression, `migrate` command

The `core-api` binary: configuration from the environment (Cloud Run sets `PORT`), mimalloc, structured logs (JSON in production), graceful shutdown on SIGTERM, liveness `/healthz` and readiness `/readyz`, request IDs and response compression. `core-api migrate` applies migrations as the schema owner, never as the API role. Errors are RFC 9457 `application/problem+json`.

**Files:**
- Create: `crates/core-api/tests/common/mod.rs`
- Create: `crates/core-api/tests/health.rs`
- Create: `crates/core-api/Cargo.toml`
- Create: `crates/core-api/src/lib.rs`
- Create: `crates/core-api/src/config.rs`
- Create: `crates/core-api/src/error.rs`
- Create: `crates/core-api/src/state.rs`
- Create: `crates/core-api/src/routes/mod.rs`
- Create: `crates/core-api/src/routes/health.rs`
- Create: `crates/core-api/src/main.rs`

**Interfaces:**
- Consumes: `db::connect`, `db::MIGRATOR`, `db::testing::app_pool`.
- Produces: `core_api::AppState { pool: PgPool, production: bool }` (`Clone`), `AppState::new(PgPool, production: bool)`; `core_api::router(AppState) -> axum::Router`.
- Produces: `core_api::error::ApiError` with constructors `bad_request(detail)`, `unauthenticated()`, `invalid_credentials()`, `forbidden(detail)`, `conflict(detail)`, `unprocessable(detail)`, `internal()`; `From<sqlx::Error>` (logs, returns 500).
- Produces (tests): `common::TestApp::new(PgConnectOptions)`, `send(method, path, cookie, body) -> TestResponse` (adds the CSRF header), `send_with(..., extra_headers)`; `TestResponse { status, headers, body: serde_json::Value }`.

- [ ] **Step 1: Write the failing tests**

Create `crates/core-api/tests/common/mod.rs`:

```rust
#![allow(dead_code)] // each test binary uses a different subset

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use core_api::{AppState, router};
use serde_json::Value;
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use tower::ServiceExt;

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub pool: PgPool,
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
}

impl TestApp {
    pub async fn new(opts: PgConnectOptions) -> Self {
        let pool = db::testing::app_pool(opts, 5).await;
        let state = AppState::new(pool.clone(), false);
        Self { router: router(state.clone()), state, pool }
    }

    /// Sends a request as the browser SPA would: JSON, with the CSRF header.
    pub async fn send(&self, method: Method, path: &str, cookie: Option<&str>, body: Option<Value>) -> TestResponse {
        self.send_with(method, path, cookie, body, &[("x-goodfolk-csrf", "1")]).await
    }

    pub async fn send_with(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        extra_headers: &[(&str, &str)],
    ) -> TestResponse {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        for (name, value) in extra_headers {
            builder = builder.header(*name, *value);
        }
        let request = match body {
            Some(body) => builder.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        TestResponse { status, headers, body }
    }
}
```

Create `crates/core-api/tests/health.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn liveness_and_readiness_report_ok(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    assert_eq!(app.send(Method::GET, "/healthz", None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.send(Method::GET, "/readyz", None, None).await.status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn responses_carry_a_request_id(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::GET, "/healthz", None, None).await;

    assert!(response.headers.contains_key("x-request-id"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test health
```

Expected: FAIL: `cargo` cannot load the new workspace member (its `Cargo.toml` does not exist yet).

- [ ] **Step 3: Implement**

Create `crates/core-api/Cargo.toml`:

```toml
[package]
name = "core-api"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true
default-run = "core-api"

[dependencies]
anyhow.workspace = true
axum.workspace = true
db.workspace = true
mimalloc.workspace = true
serde.workspace = true
sqlx.workspace = true
tokio.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
serde_json.workspace = true
tokio.workspace = true
tower.workspace = true

[lints]
workspace = true
```

Create `crates/core-api/src/lib.rs`:

```rust
//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod config;
pub mod error;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
```

Create `crates/core-api/src/config.rs`:

```rust
use anyhow::Context;
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    /// Pooled connection string (Neon pooler in production), as the `goodfolk_api` role.
    pub database_url: String,
    /// Direct (unpooled) connection string for LISTEN; poolers in transaction mode do not support it.
    pub database_listen_url: String,
    pub database_max_connections: u32,
    pub bind_addr: SocketAddr,
    pub production: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
        let database_listen_url = std::env::var("DATABASE_LISTEN_URL").unwrap_or_else(|_| database_url.clone());
        let database_max_connections = std::env::var("DATABASE_MAX_CONNECTIONS")
            .map(|v| v.parse().context("DATABASE_MAX_CONNECTIONS must be a number"))
            .unwrap_or(Ok(10))?;
        // Cloud Run provides PORT.
        let port: u16 =
            std::env::var("PORT").map(|v| v.parse().context("PORT must be a number")).unwrap_or(Ok(8080))?;
        let production = std::env::var("APP_ENV").is_ok_and(|v| v == "production");
        Ok(Self {
            database_url,
            database_listen_url,
            database_max_connections,
            bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
            production,
        })
    }
}
```

Create `crates/core-api/src/error.rs`:

```rust
use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// An RFC 9457 `application/problem+json` error.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    title: &'static str,
    detail: Option<String>,
}

#[derive(Serialize)]
struct Problem<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'a str,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

impl ApiError {
    fn new(status: StatusCode, title: &'static str, detail: Option<String>) -> Self {
        Self { status, title, detail }
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad request", Some(detail.into()))
    }

    pub fn unauthenticated() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Not signed in", None)
    }

    pub fn invalid_credentials() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Invalid email or password", None)
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "Forbidden", Some(detail.into()))
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "Conflict", Some(detail.into()))
    }

    pub fn unprocessable(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", Some(detail.into()))
    }

    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal error", None)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = %err, "database error");
        Self::internal()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Problem {
            kind: "about:blank",
            title: self.title,
            status: self.status.as_u16(),
            detail: self.detail.as_deref(),
        };
        let mut response = (self.status, Json(body)).into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/problem+json"));
        response
    }
}
```

Create `crates/core-api/src/state.rs`:

```rust
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// Production sets `Secure` cookies and disables GraphQL introspection.
    pub production: bool,
}

impl AppState {
    pub fn new(pool: PgPool, production: bool) -> Self {
        Self { pool, production }
    }
}
```

Create `crates/core-api/src/routes/mod.rs`:

```rust
mod health;

use crate::state::AppState;
use axum::Router;
use axum::routing::get;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
```

Create `crates/core-api/src/routes/health.rs`:

```rust
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;

/// Liveness: the process is serving requests.
pub async fn live() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Readiness: the database is reachable.
pub async fn ready(State(state): State<AppState>) -> StatusCode {
    match sqlx::query("select 1").execute(&state.pool).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
```

Create `crates/core-api/src/main.rs`:

```rust
use anyhow::{Context, bail};
use core_api::config::Config;
use core_api::{AppState, router};
use tracing_subscriber::EnvFilter;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve().await,
        Some("migrate") => migrate().await,
        Some(other) => bail!("unknown command `{other}`; expected `serve` or `migrate`"),
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(config.production);
    let pool = db::connect(&config.database_url, config.database_max_connections).await?;
    let state = AppState::new(pool, config.production);

    let tcp = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "listening");
    axum::serve(tcp, router(state)).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

/// Runs migrations as the schema owner (`DATABASE_OWNER_URL`), never as the API role.
async fn migrate() -> anyhow::Result<()> {
    init_tracing(false);
    let url = std::env::var("DATABASE_OWNER_URL").context("DATABASE_OWNER_URL is required")?;
    let pool = db::connect(&url, 1).await?;
    db::MIGRATOR.run(&pool).await?;
    tracing::info!("migrations applied");
    Ok(())
}

fn init_tracing(json: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tower_http=info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if json { builder.json().init() } else { builder.init() }
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("install Ctrl+C handler") };
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 2 tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Run it for real**

Apply migrations as the owner, then start the API as `goodfolk_api`:

```sh
export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
export DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk
cargo run -p core-api -- migrate
cargo run -p core-api &
curl -si localhost:8080/readyz | head -1
```

Expected: `HTTP/1.1 204 No Content`. Stop the server afterwards (`kill %1`).

- [ ] **Step 6: Commit**

```sh
git add -A
git commit -m "feat(core-api): service skeleton with health checks and migrate command"
```

### Task 8: Sign-up, login and sessions over HTTP (with CSRF protection)

REST endpoints for sign-up, login, logout, current profile and tenant switching. The session token lives in an `HttpOnly; SameSite=Lax` cookie (`Secure` in production). Every state-changing request must carry the `x-goodfolk-csrf` header: browsers cannot add custom headers cross-site without a CORS preflight, which we never allow. Extractors resolve the session once per request and cache it in request extensions.

**Files:**
- Modify: `crates/core-api/tests/common/mod.rs`
- Create: `crates/core-api/tests/auth.rs`
- Modify: `crates/core-api/Cargo.toml`
- Modify: `crates/core-api/src/lib.rs`
- Modify: `crates/core-api/src/error.rs`
- Create: `crates/core-api/src/auth.rs`
- Create: `crates/core-api/src/csrf.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Create: `crates/core-api/src/routes/auth.rs`

**Interfaces:**
- Consumes: everything `identity` exports from Tasks 4–5.
- Produces: extractors `core_api::auth::Authenticated { session: SessionInfo, token: String }` (401 if missing) and `core_api::auth::TenantContext { user: UserId, tenant: TenantId, grants: Vec<Grant> }` (403 if no tenant selected) with `require(Permission, Option<Uuid>) -> Result<(), ApiError>`.
- Produces: `core_api::error::validate<T: garde::Validate<Context = ()>>(&T) -> Result<(), ApiError>` (422 on failure); `core_api::csrf::CSRF_HEADER = "x-goodfolk-csrf"`.
- Produces routes: `POST /api/v1/auth/signup` (201 + cookie + `Profile`), `POST /api/v1/auth/login`, `POST /api/v1/auth/logout` (204), `GET /api/v1/me`, `PUT /api/v1/session/tenant`.
- Produces (tests): `TestApp::signup_owner(email, tenant_name) -> String` (cookie `name=value`), `common::session_cookie(&HeaderMap) -> String`.

- [ ] **Step 1: Write the failing tests**

Replace the whole file `crates/core-api/tests/common/mod.rs`:

```rust
#![allow(dead_code)] // each test binary uses a different subset

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use core_api::{AppState, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use tower::ServiceExt;

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub pool: PgPool,
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
}

impl TestApp {
    pub async fn new(opts: PgConnectOptions) -> Self {
        let pool = db::testing::app_pool(opts, 5).await;
        let state = AppState::new(pool.clone(), false);
        Self { router: router(state.clone()), state, pool }
    }

    /// Sends a request as the browser SPA would: JSON, with the CSRF header.
    pub async fn send(&self, method: Method, path: &str, cookie: Option<&str>, body: Option<Value>) -> TestResponse {
        self.send_with(method, path, cookie, body, &[("x-goodfolk-csrf", "1")]).await
    }

    pub async fn send_with(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        extra_headers: &[(&str, &str)],
    ) -> TestResponse {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        for (name, value) in extra_headers {
            builder = builder.header(*name, *value);
        }
        let request = match body {
            Some(body) => builder.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        TestResponse { status, headers, body }
    }

    /// Signs up a new owner and returns their session cookie (`name=value`).
    pub async fn signup_owner(&self, email: &str, tenant_name: &str) -> String {
        let response = self
            .send(
                Method::POST,
                "/api/v1/auth/signup",
                None,
                Some(json!({
                    "email": email,
                    "password": "a long enough password",
                    "display_name": "Owner",
                    "tenant_name": tenant_name,
                })),
            )
            .await;
        assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
        session_cookie(&response.headers)
    }
}

pub fn session_cookie(headers: &HeaderMap) -> String {
    let set_cookie = headers.get(header::SET_COOKIE).expect("Set-Cookie header").to_str().unwrap();
    set_cookie.split(';').next().unwrap().to_owned()
}
```

Create `crates/core-api/tests/auth.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, session_cookie};
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn signup_sets_a_secure_session_cookie_and_returns_the_profile(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app
        .send(
            Method::POST,
            "/api/v1/auth/signup",
            None,
            Some(json!({"email": "owner@example.com", "password": "a long enough password",
                        "display_name": "Owner", "tenant_name": "Lagoon Hotels"})),
        )
        .await;

    assert_eq!(response.status, StatusCode::CREATED);
    let cookie = response.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.starts_with("gf_session="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    assert_eq!(response.body["email"], "owner@example.com");
    assert_eq!(response.body["tenants"][0]["name"], "Lagoon Hotels");
    assert_eq!(response.body["grants"][0]["role"], "owner");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn signup_rejects_short_passwords_and_duplicate_emails(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    app.signup_owner("owner@example.com", "A").await;

    let short = app
        .send(
            Method::POST,
            "/api/v1/auth/signup",
            None,
            Some(json!({"email": "new@example.com", "password": "short", "display_name": "N", "tenant_name": "B"})),
        )
        .await;
    let duplicate = app
        .send(
            Method::POST,
            "/api/v1/auth/signup",
            None,
            Some(json!({"email": "OWNER@example.com", "password": "a long enough password",
                        "display_name": "N", "tenant_name": "B"})),
        )
        .await;

    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(short.headers.get(header::CONTENT_TYPE).unwrap(), "application/problem+json");
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn login_me_and_logout(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    app.signup_owner("owner@example.com", "A").await;

    let wrong = app
        .send(
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({"email": "owner@example.com", "password": "not the password"})),
        )
        .await;
    let login = app
        .send(
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({"email": "owner@example.com", "password": "a long enough password"})),
        )
        .await;
    let cookie = session_cookie(&login.headers);
    let me = app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await;
    let logout = app.send(Method::POST, "/api/v1/auth/logout", Some(&cookie), None).await;
    let after = app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await;

    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login.status, StatusCode::OK);
    assert_eq!(me.body["email"], "owner@example.com");
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn state_changing_requests_need_the_csrf_header(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app
        .send_with(
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({"email": "a@example.com", "password": "x"})),
            &[],
        )
        .await;

    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_user_cannot_switch_into_a_tenant_they_do_not_belong_to(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;
    let bobs_tenant = app.send(Method::GET, "/api/v1/me", Some(&bob), None).await.body["current_tenant"].clone();

    let response =
        app.send(Method::PUT, "/api/v1/session/tenant", Some(&alice), Some(json!({"tenant_id": bobs_tenant}))).await;

    assert_eq!(response.status, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test auth
```

Expected: FAIL: the tests compile but fail: every `/api/v1/auth/*` request returns 404.

- [ ] **Step 3: Implement**

Replace the whole file `crates/core-api/Cargo.toml`:

```toml
[package]
name = "core-api"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true
default-run = "core-api"

[dependencies]
anyhow.workspace = true
axum.workspace = true
axum-extra.workspace = true
db.workspace = true
garde.workspace = true
identity.workspace = true
mimalloc.workspace = true
serde.workspace = true
sqlx.workspace = true
time.workspace = true
tokio.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
utoipa.workspace = true
uuid.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
serde_json.workspace = true
tokio.workspace = true
tower.workspace = true

[lints]
workspace = true
```

Replace the whole file `crates/core-api/src/lib.rs`:

```rust
//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod auth;
pub mod config;
pub mod csrf;
pub mod error;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
```

Replace the whole file `crates/core-api/src/error.rs`:

```rust
use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// An RFC 9457 `application/problem+json` error.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    title: &'static str,
    detail: Option<String>,
}

#[derive(Serialize)]
struct Problem<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'a str,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

impl ApiError {
    fn new(status: StatusCode, title: &'static str, detail: Option<String>) -> Self {
        Self { status, title, detail }
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad request", Some(detail.into()))
    }

    pub fn unauthenticated() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Not signed in", None)
    }

    pub fn invalid_credentials() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Invalid email or password", None)
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "Forbidden", Some(detail.into()))
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "Conflict", Some(detail.into()))
    }

    pub fn unprocessable(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", Some(detail.into()))
    }

    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal error", None)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = %err, "database error");
        Self::internal()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Problem {
            kind: "about:blank",
            title: self.title,
            status: self.status.as_u16(),
            detail: self.detail.as_deref(),
        };
        let mut response = (self.status, Json(body)).into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/problem+json"));
        response
    }
}

/// Validates a request DTO, turning the report into a 422.
pub fn validate<T: garde::Validate<Context = ()>>(value: &T) -> Result<(), ApiError> {
    value.validate().map_err(|report| ApiError::unprocessable(report.to_string()))
}
```

Create `crates/core-api/src/auth.rs`:

```rust
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use db::{Scope, TenantId, UserId};
use identity::{Grant, Permission, SessionInfo, allows, load_grants, resolve_session};
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "gf_session";

pub fn session_cookie(token: String, expires: time::OffsetDateTime, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .expires(expires)
        .build()
}

pub fn removal_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, "")).path("/").build()
}

/// A signed-in user, from the session cookie.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub session: SessionInfo,
    pub token: String,
}

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(found) = parts.extensions.get::<Authenticated>() {
            return Ok(found.clone());
        }
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_owned()).ok_or_else(ApiError::unauthenticated)?;
        let session = resolve_session(&state.pool, &token).await?.ok_or_else(ApiError::unauthenticated)?;
        let found = Authenticated { session, token };
        parts.extensions.insert(found.clone());
        Ok(found)
    }
}

/// A signed-in user acting in their session's current tenant, with their grants loaded.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub user: UserId,
    pub tenant: TenantId,
    pub grants: Vec<Grant>,
}

impl TenantContext {
    pub fn require(&self, permission: Permission, property: Option<Uuid>) -> Result<(), ApiError> {
        if allows(&self.grants, permission, property) {
            Ok(())
        } else {
            Err(ApiError::forbidden("you do not have permission for this action"))
        }
    }
}

impl FromRequestParts<AppState> for TenantContext {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(found) = parts.extensions.get::<TenantContext>() {
            return Ok(found.clone());
        }
        let auth = Authenticated::from_request_parts(parts, state).await?;
        let tenant = auth.session.tenant.ok_or_else(|| ApiError::forbidden("no tenant selected"))?;
        let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
        let grants = load_grants(&mut tx, auth.session.user).await?;
        tx.commit().await?;
        let found = TenantContext { user: auth.session.user, tenant, grants };
        parts.extensions.insert(found.clone());
        Ok(found)
    }
}
```

Create `crates/core-api/src/csrf.rs`:

```rust
use crate::error::ApiError;
use axum::extract::Request;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Browsers cannot send custom headers cross-site without a CORS preflight, which we never allow,
/// so requiring one on every state-changing request blocks CSRF (on top of `SameSite=Lax`).
pub const CSRF_HEADER: &str = "x-goodfolk-csrf";

pub async fn require_csrf_header(request: Request, next: Next) -> Response {
    let safe = matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if safe || request.headers().contains_key(CSRF_HEADER) {
        next.run(request).await
    } else {
        ApiError::forbidden(format!("missing {CSRF_HEADER} header")).into_response()
    }
}
```

Replace the whole file `crates/core-api/src/routes/mod.rs`:

```rust
pub(crate) mod auth;
mod health;

use crate::csrf;
use crate::state::AppState;
use axum::Router;
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::routing::{get, post, put};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};

pub fn router(state: AppState) -> Router {
    let requests = Router::new()
        .route("/api/v1/auth/signup", post(auth::signup))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/me", get(auth::me))
        .route("/api/v1/session/tenant", put(auth::switch_tenant))
        .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(15)));

    Router::new()
        .merge(requests)
        .layer(from_fn(csrf::require_csrf_header))
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
```

Create `crates/core-api/src/routes/auth.rs`:

```rust
use crate::auth::{Authenticated, removal_cookie, session_cookie};
use crate::error::{ApiError, validate};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use db::TenantId;
use garde::Validate;
use identity::{Profile, SignupError, SignupInput};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SignupRequest {
    #[garde(email)]
    pub email: String,
    #[garde(length(chars, min = 12, max = 128))]
    pub password: String,
    #[garde(length(chars, min = 1, max = 200))]
    pub display_name: String,
    #[garde(length(chars, min = 1, max = 200))]
    pub tenant_name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SwitchTenantRequest {
    pub tenant_id: Uuid,
}

#[utoipa::path(post, path = "/api/v1/auth/signup", request_body = SignupRequest,
    responses((status = 201, body = Profile), (status = 409), (status = 422)))]
pub async fn signup(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<SignupRequest>,
) -> Result<(StatusCode, CookieJar, Json<Profile>), ApiError> {
    validate(&body)?;
    let input = SignupInput {
        email: body.email,
        password: body.password,
        display_name: body.display_name,
        tenant_name: body.tenant_name,
    };
    let (user, tenant) = identity::signup(&state.pool, input).await.map_err(|err| match err {
        SignupError::EmailTaken => ApiError::conflict(err.to_string()),
        SignupError::Database(db_err) => db_err.into(),
    })?;
    let (token, expires) = identity::create_session(&state.pool, user, Some(tenant)).await?;
    let profile = identity::load_profile(&state.pool, user, Some(tenant)).await?;
    Ok((StatusCode::CREATED, jar.add(session_cookie(token, expires, state.production)), Json(profile)))
}

#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest,
    responses((status = 200, body = Profile), (status = 401)))]
pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<LoginRequest>,
) -> Result<(CookieJar, Json<Profile>), ApiError> {
    let user = identity::authenticate(&state.pool, &body.email, &body.password)
        .await?
        .ok_or_else(ApiError::invalid_credentials)?;
    let tenant = identity::default_tenant(&state.pool, user).await?;
    let (token, expires) = identity::create_session(&state.pool, user, tenant).await?;
    let profile = identity::load_profile(&state.pool, user, tenant).await?;
    Ok((jar.add(session_cookie(token, expires, state.production)), Json(profile)))
}

#[utoipa::path(post, path = "/api/v1/auth/logout", responses((status = 204), (status = 401)))]
pub async fn logout(
    State(state): State<AppState>,
    auth: Authenticated,
    jar: CookieJar,
) -> Result<(StatusCode, CookieJar), ApiError> {
    identity::delete_session(&state.pool, &auth.token).await?;
    Ok((StatusCode::NO_CONTENT, jar.remove(removal_cookie())))
}

#[utoipa::path(get, path = "/api/v1/me", responses((status = 200, body = Profile), (status = 401)))]
pub async fn me(State(state): State<AppState>, auth: Authenticated) -> Result<Json<Profile>, ApiError> {
    Ok(Json(identity::load_profile(&state.pool, auth.session.user, auth.session.tenant).await?))
}

#[utoipa::path(put, path = "/api/v1/session/tenant", request_body = SwitchTenantRequest,
    responses((status = 200, body = Profile), (status = 403)))]
pub async fn switch_tenant(
    State(state): State<AppState>,
    auth: Authenticated,
    Json(body): Json<SwitchTenantRequest>,
) -> Result<Json<Profile>, ApiError> {
    let tenant = TenantId(body.tenant_id);
    if !identity::switch_tenant(&state.pool, &auth.session, tenant).await? {
        return Err(ApiError::forbidden("you are not a member of this tenant"));
    }
    Ok(Json(identity::load_profile(&state.pool, auth.session.user, Some(tenant)).await?))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 7 tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(core-api): sign-up, login, logout and tenant switching with CSRF protection"
```

### Task 9: Create-property command with idempotency keys

`POST /api/v1/properties` requires an `Idempotency-Key` header. The middleware claims the key before the handler runs, stores the first response, and replays it for retries of the same request. A different request with the same key is rejected (422); a retry while the first is still running gets 409; server errors are not stored, so they can be retried.

**Files:**
- Create: `crates/core-api/tests/properties.rs`
- Modify: `crates/core-api/Cargo.toml`
- Modify: `crates/core-api/src/lib.rs`
- Create: `crates/core-api/src/idempotency.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Create: `crates/core-api/src/routes/properties.rs`

**Interfaces:**
- Consumes: `TenantContext` (Task 8), `property::create_property` (Task 6).
- Produces: `core_api::idempotency::idempotent` (axum middleware, `from_fn_with_state`), `IDEMPOTENCY_HEADER = "idempotency-key"`. Apply it with `.route_layer` to every create command in later phases.
- Produces route: `POST /api/v1/properties` (201 `Property`; 403 without `PropertiesCreate`; 409 duplicate code; 422 invalid fields or unknown time zone).

- [ ] **Step 1: Write the failing test**

Create `crates/core-api/tests/properties.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

fn galle() -> Value {
    json!({"code": "GAL", "name": "Galle Fort Hotel", "timezone": "Asia/Colombo", "base_currency": "LKR"})
}

async fn create(app: &TestApp, cookie: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(
        Method::POST,
        "/api/v1/properties",
        Some(cookie),
        Some(body),
        &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)],
    )
    .await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_owner_creates_a_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let created = create(&app, &owner, "key-00000001", galle()).await;

    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["code"], "GAL");
    assert_eq!(created.body["base_currency"], "LKR");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn property_codes_are_unique_within_a_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;
    create(&app, &alice, "key-00000001", galle()).await;

    let same_tenant = create(&app, &alice, "key-00000002", galle()).await;
    let other_tenant = create(&app, &bob, "key-00000001", galle()).await;

    assert_eq!(same_tenant.status, StatusCode::CONFLICT);
    assert_eq!(other_tenant.status, StatusCode::CREATED);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn retrying_with_the_same_idempotency_key_replays_the_first_response(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let first = create(&app, &owner, "key-00000001", galle()).await;
    let retry = create(&app, &owner, "key-00000001", galle()).await;
    let kandy = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let reused = create(&app, &owner, "key-00000001", kandy).await;

    assert_eq!(retry.status, StatusCode::CREATED);
    assert_eq!(retry.body, first.body);
    assert_eq!(reused.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn creating_requires_an_idempotency_key_and_valid_fields(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let no_key = app.send(Method::POST, "/api/v1/properties", Some(&owner), Some(galle())).await;
    let bad_code = json!({"code": "g", "name": "G", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let bad_code = create(&app, &owner, "key-00000001", bad_code).await;
    let bad_zone = json!({"code": "GAL", "name": "G", "timezone": "Mars/Base", "base_currency": "LKR"});
    let bad_zone = create(&app, &owner, "key-00000002", bad_zone).await;

    assert_eq!(no_key.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad_code.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad_zone.status, StatusCode::UNPROCESSABLE_ENTITY);
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test properties
```

Expected: FAIL: the tests compile but fail: `/api/v1/properties` returns 404.

- [ ] **Step 3: Implement**

Replace the whole file `crates/core-api/Cargo.toml`:

```toml
[package]
name = "core-api"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true
default-run = "core-api"

[dependencies]
anyhow.workspace = true
axum.workspace = true
axum-extra.workspace = true
db.workspace = true
garde.workspace = true
identity.workspace = true
mimalloc.workspace = true
property.workspace = true
serde.workspace = true
sha2.workspace = true
sqlx.workspace = true
time.workspace = true
tokio.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
utoipa.workspace = true
uuid.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
serde_json.workspace = true
tokio.workspace = true
tower.workspace = true

[lints]
workspace = true
```

Replace the whole file `crates/core-api/src/lib.rs`:

```rust
//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod auth;
pub mod config;
pub mod csrf;
pub mod error;
pub mod idempotency;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
```

Create `crates/core-api/src/idempotency.rs`:

```rust
use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::state::AppState;
use axum::body::{Body, to_bytes};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use db::Scope;
use sha2::{Digest, Sha256};

pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Makes create commands safe to retry. The first response for an `Idempotency-Key` is stored
/// and replayed for retries of the same request. Server errors are not stored, so they can be retried.
pub async fn idempotent(State(state): State<AppState>, request: Request, next: Next) -> Result<Response, ApiError> {
    let key = request
        .headers()
        .get(IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| (8..=200).contains(&v.len()))
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("an Idempotency-Key header of 8 to 200 characters is required"))?;

    let (mut parts, body) = request.into_parts();
    let ctx = TenantContext::from_request_parts(&mut parts, &state).await?;
    let bytes = to_bytes(body, MAX_BODY_BYTES).await.map_err(|_| ApiError::bad_request("request body too large"))?;
    let mut hasher = Sha256::new();
    hasher.update(parts.method.as_str());
    hasher.update(parts.uri.path());
    hasher.update(&bytes);
    let request_hash = hasher.finalize().to_vec();

    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let claimed = sqlx::query(
        "insert into idempotency_key (tenant_id, key, request_hash) values ($1, $2, $3) on conflict do nothing",
    )
    .bind(ctx.tenant.0)
    .bind(&key)
    .bind(&request_hash)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        let (stored_hash, status, body): (Vec<u8>, Option<i16>, Option<Vec<u8>>) = sqlx::query_as(
            "select request_hash, status_code, response_body from idempotency_key where tenant_id = $1 and key = $2",
        )
        .bind(ctx.tenant.0)
        .bind(&key)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        if stored_hash != request_hash {
            return Err(ApiError::unprocessable("Idempotency-Key was already used for a different request"));
        }
        let (Some(status), Some(body)) = (status, body) else {
            return Err(ApiError::conflict("a request with this Idempotency-Key is still in progress"));
        };
        let status =
            u16::try_from(status).ok().and_then(|s| StatusCode::from_u16(s).ok()).ok_or_else(ApiError::internal)?;
        return Ok((status, [(header::CONTENT_TYPE, "application/json")], body).into_response());
    }
    tx.commit().await?;

    let response = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let (response_parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.map_err(|_| ApiError::internal())?;

    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    if response_parts.status.is_server_error() {
        sqlx::query("delete from idempotency_key where tenant_id = $1 and key = $2")
            .bind(ctx.tenant.0)
            .bind(&key)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query(
            "update idempotency_key set status_code = $3, response_body = $4 where tenant_id = $1 and key = $2",
        )
        .bind(ctx.tenant.0)
        .bind(&key)
        .bind(i16::try_from(response_parts.status.as_u16()).map_err(|_| ApiError::internal())?)
        .bind(body.to_vec())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Response::from_parts(response_parts, Body::from(body)))
}
```

Replace the whole file `crates/core-api/src/routes/mod.rs`:

```rust
pub(crate) mod auth;
mod health;
pub(crate) mod properties;

use crate::state::AppState;
use crate::{csrf, idempotency};
use axum::Router;
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post, put};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
pub use properties::CreatePropertyRequest;

pub fn router(state: AppState) -> Router {
    let commands = Router::new()
        .route("/api/v1/properties", post(properties::create))
        .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));

    let requests = Router::new()
        .route("/api/v1/auth/signup", post(auth::signup))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/me", get(auth::me))
        .route("/api/v1/session/tenant", put(auth::switch_tenant))
        .merge(commands)
        .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(15)));

    Router::new()
        .merge(requests)
        .layer(from_fn(csrf::require_csrf_header))
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
```

Create `crates/core-api/src/routes/properties.rs`:

```rust
use crate::auth::TenantContext;
use crate::error::{ApiError, validate};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use property::{NewProperty, Property, PropertyError};
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreatePropertyRequest {
    /// 2 to 10 capital letters or digits, unique within the tenant.
    #[garde(pattern(r"^[A-Z0-9]{2,10}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 200))]
    pub name: String,
    /// IANA time zone, e.g. `Asia/Colombo`.
    #[garde(length(min = 1, max = 64))]
    pub timezone: String,
    /// ISO 4217 code, e.g. `LKR`.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub base_currency: String,
}

#[utoipa::path(post, path = "/api/v1/properties", request_body = CreatePropertyRequest,
    params(("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Property), (status = 403), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    Json(body): Json<CreatePropertyRequest>,
) -> Result<(StatusCode, Json<Property>), ApiError> {
    ctx.require(Permission::PropertiesCreate, None)?;
    validate(&body)?;
    let input =
        NewProperty { code: body.code, name: body.name, timezone: body.timezone, base_currency: body.base_currency };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = property::create_property(&mut tx, ctx.tenant, ctx.user, input).await.map_err(|err| match err {
        PropertyError::CodeTaken => ApiError::conflict(err.to_string()),
        PropertyError::UnknownTimezone => ApiError::unprocessable(err.to_string()),
        PropertyError::Database(db_err) => db_err.into(),
    })?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(created)))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 11 tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(core-api): create-property command with idempotency keys"
```

### Task 10: GraphQL read API

Reads go through GraphQL at `POST /graphql`; it has no mutations. The schema has depth (8) and complexity (500) limits and hides introspection in production. The pool and the caller's `TenantContext` are attached per request. Staff with only property-scoped grants see only those properties.

**Files:**
- Create: `crates/core-api/tests/graphql.rs`
- Modify: `crates/core-api/Cargo.toml`
- Modify: `crates/core-api/src/lib.rs`
- Create: `crates/core-api/src/graphql.rs`
- Modify: `crates/core-api/src/state.rs`
- Modify: `crates/core-api/src/auth.rs`
- Modify: `crates/core-api/src/routes/mod.rs`

**Interfaces:**
- Consumes: `property::list_properties`, `TenantContext`.
- Produces: `core_api::graphql::{GqlSchema, build_schema(production: bool) -> GqlSchema, handler}`; `AppState` gains `schema: GqlSchema`; `TenantContext::visible_properties() -> Option<Vec<Uuid>>` (`None` = all).
- Produces GraphQL: `type Query { properties: [PropertyNode!]! }`, `PropertyNode { id: UUID!, code, name, timezone, baseCurrency }`.

- [ ] **Step 1: Write the failing test**

Create `crates/core-api/tests/graphql.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use core_api::graphql::build_schema;
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

async fn create(app: &TestApp, cookie: &str, code: &str) {
    let body =
        json!({"code": code, "name": format!("Hotel {code}"), "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let key = format!("key-create-{code}");
    let response = app
        .send_with(
            Method::POST,
            "/api/v1/properties",
            Some(cookie),
            Some(body),
            &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)],
        )
        .await;
    assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
}

async fn list(app: &TestApp, cookie: &str) -> Value {
    let query = json!({"query": "{ properties { code name } }"});
    let response = app.send(Method::POST, "/graphql", Some(cookie), Some(query)).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body["data"]["properties"].clone()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn properties_are_listed_by_code(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    create(&app, &owner, "KAN").await;
    create(&app, &owner, "GAL").await;

    assert_eq!(
        list(&app, &owner).await,
        json!([{"code": "GAL", "name": "Hotel GAL"}, {"code": "KAN", "name": "Hotel KAN"}])
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tenants_never_see_each_others_properties(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;

    create(&app, &alice, "GAL").await;

    assert_eq!(list(&app, &bob).await, json!([]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn graphql_requires_a_session(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::POST, "/graphql", None, Some(json!({"query": "{ properties { code } }"}))).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn production_hides_the_schema_from_introspection() {
    let query = "{ __schema { queryType { name } } }";

    let development = build_schema(false).execute(query).await.data.into_json().unwrap();
    let production = build_schema(true).execute(query).await.data.into_json().unwrap();

    assert_eq!(development, json!({"__schema": {"queryType": {"name": "Query"}}}));
    assert_eq!(production, json!({"__schema": null}));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test graphql
```

Expected: FAIL: compile error, `core_api::graphql` not found.

- [ ] **Step 3: Implement**

Replace the whole file `crates/core-api/Cargo.toml`:

```toml
[package]
name = "core-api"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true
default-run = "core-api"

[dependencies]
anyhow.workspace = true
async-graphql.workspace = true
async-graphql-axum.workspace = true
axum.workspace = true
axum-extra.workspace = true
db.workspace = true
garde.workspace = true
identity.workspace = true
mimalloc.workspace = true
property.workspace = true
serde.workspace = true
sha2.workspace = true
sqlx.workspace = true
time.workspace = true
tokio.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
utoipa.workspace = true
uuid.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
serde_json.workspace = true
tokio.workspace = true
tower.workspace = true

[lints]
workspace = true
```

Replace the whole file `crates/core-api/src/lib.rs`:

```rust
//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod auth;
pub mod config;
pub mod csrf;
pub mod error;
pub mod graphql;
pub mod idempotency;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
```

Create `crates/core-api/src/graphql.rs`:

```rust
//! Read-only GraphQL API. All writes go through REST commands.

use crate::auth::TenantContext;
use crate::state::AppState;
use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::extract::State;
use db::Scope;
use sqlx::PgPool;
use uuid::Uuid;

pub type GqlSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub fn build_schema(production: bool) -> GqlSchema {
    let builder = Schema::build(Query, EmptyMutation, EmptySubscription).limit_depth(8).limit_complexity(500);
    if production { builder.disable_introspection().finish() } else { builder.finish() }
}

pub async fn handler(State(state): State<AppState>, ctx: TenantContext, request: GraphQLRequest) -> GraphQLResponse {
    state.schema.execute(request.into_inner().data(state.pool.clone()).data(ctx)).await.into()
}

#[derive(SimpleObject)]
pub struct PropertyNode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
}

pub struct Query;

#[Object]
impl Query {
    /// Properties the current user can see, ordered by code.
    async fn properties(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<PropertyNode>> {
        let pool = ctx.data::<PgPool>()?;
        let tenant = ctx.data::<TenantContext>()?;
        let visible = tenant.visible_properties();
        let mut tx = db::begin(pool, Scope::tenant(tenant.tenant)).await?;
        let properties = property::list_properties(&mut tx, visible.as_deref()).await?;
        tx.commit().await?;
        Ok(properties
            .into_iter()
            .map(|p| PropertyNode {
                id: p.id,
                code: p.code,
                name: p.name,
                timezone: p.timezone,
                base_currency: p.base_currency,
            })
            .collect())
    }
}
```

Replace the whole file `crates/core-api/src/state.rs`:

```rust
use crate::graphql::{GqlSchema, build_schema};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub schema: GqlSchema,
    /// Production sets `Secure` cookies and disables GraphQL introspection.
    pub production: bool,
}

impl AppState {
    pub fn new(pool: PgPool, production: bool) -> Self {
        Self { schema: build_schema(production), pool, production }
    }
}
```

Replace the whole file `crates/core-api/src/auth.rs`:

```rust
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use db::{Scope, TenantId, UserId};
use identity::{Grant, Permission, SessionInfo, allows, load_grants, resolve_session};
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "gf_session";

pub fn session_cookie(token: String, expires: time::OffsetDateTime, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .expires(expires)
        .build()
}

pub fn removal_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, "")).path("/").build()
}

/// A signed-in user, from the session cookie.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub session: SessionInfo,
    pub token: String,
}

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(found) = parts.extensions.get::<Authenticated>() {
            return Ok(found.clone());
        }
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_owned()).ok_or_else(ApiError::unauthenticated)?;
        let session = resolve_session(&state.pool, &token).await?.ok_or_else(ApiError::unauthenticated)?;
        let found = Authenticated { session, token };
        parts.extensions.insert(found.clone());
        Ok(found)
    }
}

/// A signed-in user acting in their session's current tenant, with their grants loaded.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub user: UserId,
    pub tenant: TenantId,
    pub grants: Vec<Grant>,
}

impl TenantContext {
    pub fn require(&self, permission: Permission, property: Option<Uuid>) -> Result<(), ApiError> {
        if allows(&self.grants, permission, property) {
            Ok(())
        } else {
            Err(ApiError::forbidden("you do not have permission for this action"))
        }
    }

    /// `None` if the user may see every property, otherwise the properties they hold grants for.
    pub fn visible_properties(&self) -> Option<Vec<Uuid>> {
        if allows(&self.grants, Permission::PropertiesView, None) {
            None
        } else {
            Some(self.grants.iter().filter_map(|g| g.property_id).collect())
        }
    }
}

impl FromRequestParts<AppState> for TenantContext {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(found) = parts.extensions.get::<TenantContext>() {
            return Ok(found.clone());
        }
        let auth = Authenticated::from_request_parts(parts, state).await?;
        let tenant = auth.session.tenant.ok_or_else(|| ApiError::forbidden("no tenant selected"))?;
        let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
        let grants = load_grants(&mut tx, auth.session.user).await?;
        tx.commit().await?;
        let found = TenantContext { user: auth.session.user, tenant, grants };
        parts.extensions.insert(found.clone());
        Ok(found)
    }
}
```

Replace the whole file `crates/core-api/src/routes/mod.rs`:

```rust
pub(crate) mod auth;
mod health;
pub(crate) mod properties;

use crate::state::AppState;
use crate::{csrf, graphql, idempotency};
use axum::Router;
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post, put};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
pub use properties::CreatePropertyRequest;

pub fn router(state: AppState) -> Router {
    let commands = Router::new()
        .route("/api/v1/properties", post(properties::create))
        .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));

    let requests = Router::new()
        .route("/api/v1/auth/signup", post(auth::signup))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/me", get(auth::me))
        .route("/api/v1/session/tenant", put(auth::switch_tenant))
        .route("/graphql", post(graphql::handler))
        .merge(commands)
        .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(15)));

    Router::new()
        .merge(requests)
        .layer(from_fn(csrf::require_csrf_header))
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 15 tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(core-api): read-only GraphQL API with query limits"
```

### Task 11: Live updates over server-sent events

`GET /api/v1/events` streams change events for the caller's tenant (optionally one property). Each instance runs one `LISTEN` connection (a direct, unpooled URL, since Neon's pooler does not support `LISTEN`) and fans out through a broadcast channel. The stream opens with a `ready` event, because some proxies (including Vite's dev proxy) hold response headers until the first bytes arrive. A subscriber that falls behind gets `resync`. The stream sits outside the request timeout.

**Files:**
- Create: `crates/core-api/tests/events.rs`
- Modify: `crates/core-api/Cargo.toml`
- Modify: `crates/core-api/src/lib.rs`
- Create: `crates/core-api/src/events.rs`
- Modify: `crates/core-api/src/state.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Modify: `crates/core-api/src/main.rs`

**Interfaces:**
- Consumes: `db::{CHANNEL, Event}`.
- Produces: `core_api::events::spawn_listener(PgListener, broadcast::Sender<Event>) -> JoinHandle<()>`, `core_api::events::stream` (handler); `AppState` gains `events: broadcast::Sender<Event>` (capacity 1024).
- Produces SSE events: `ready` (`{}`), `invalidate` (JSON array of cache keys), `resync` (`{}`).

- [ ] **Step 1: Write the failing test**

Create `crates/core-api/tests/events.rs`:

```rust
mod common;

use axum::body::Body;
use axum::http::{Method, Request, header};
use common::TestApp;
use core_api::events::spawn_listener;
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::time::Duration;
use tower::ServiceExt;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn creating_a_property_pushes_an_invalidation_to_the_tenants_stream(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let mut listener = PgListener::connect_with(&sqlx::PgPool::connect_with(opts).await.unwrap()).await.unwrap();
    listener.listen(db::CHANNEL).await.unwrap();
    spawn_listener(listener, app.state.events.clone());
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let request = Request::builder().uri("/api/v1/events").header(header::COOKIE, &owner).body(Body::empty()).unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/event-stream");
    let mut body = response.into_body();
    let ready = tokio::time::timeout(Duration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(ready.into_data().unwrap(), "event: ready\ndata: {}\n\n");

    app.send_with(
        Method::POST,
        "/api/v1/properties",
        Some(&owner),
        Some(json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"})),
        &[("x-goodfolk-csrf", "1"), ("idempotency-key", "key-00000001")],
    )
    .await;

    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap().unwrap().unwrap();
    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
    assert_eq!(text, "event: invalidate\ndata: [\"properties\"]\n\n");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test events
```

Expected: FAIL: compile error, `core_api::events` not found.

- [ ] **Step 3: Implement**

Replace the whole file `crates/core-api/Cargo.toml`:

```toml
[package]
name = "core-api"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true
default-run = "core-api"

[dependencies]
anyhow.workspace = true
async-graphql.workspace = true
async-graphql-axum.workspace = true
axum.workspace = true
axum-extra.workspace = true
db.workspace = true
futures.workspace = true
garde.workspace = true
identity.workspace = true
mimalloc.workspace = true
property.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
sqlx.workspace = true
time.workspace = true
tokio.workspace = true
tokio-stream.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
utoipa.workspace = true
uuid.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
http-body-util.workspace = true
tokio.workspace = true
tower.workspace = true

[lints]
workspace = true
```

Replace the whole file `crates/core-api/src/lib.rs`:

```rust
//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod auth;
pub mod config;
pub mod csrf;
pub mod error;
pub mod events;
pub mod graphql;
pub mod idempotency;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
```

Create `crates/core-api/src/events.rs`:

```rust
use crate::auth::TenantContext;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use db::Event;
use futures::Stream;
use serde::Deserialize;
use sqlx::postgres::PgListener;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use uuid::Uuid;

/// Forwards every `NOTIFY gf_events` from Postgres to this instance's subscribers.
/// Every instance runs one, so every connected client hears about every change.
pub fn spawn_listener(mut listener: PgListener, events: broadcast::Sender<Event>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match listener.recv().await {
                Ok(notification) => match serde_json::from_str::<Event>(notification.payload()) {
                    Ok(event) => {
                        // No subscribers is normal; nothing to do.
                        let _ = events.send(event);
                    }
                    Err(err) => tracing::warn!(error = %err, "ignoring malformed event payload"),
                },
                Err(err) => {
                    // PgListener reconnects on the next recv; back off briefly.
                    tracing::warn!(error = %err, "event listener error");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    })
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub property: Option<Uuid>,
}

/// `ready` is sent immediately so proxies that hold headers until the first bytes forward the stream.
/// `invalidate` events carry the cache keys that changed. `resync` means events were missed and
/// the client should refetch everything on screen.
pub async fn stream(
    State(state): State<AppState>,
    ctx: TenantContext,
    Query(query): Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let tenant = ctx.tenant;
    let ready = tokio_stream::once(Ok(SseEvent::default().event("ready").data("{}")));
    let updates = BroadcastStream::new(state.events.subscribe()).filter_map(move |item| match item {
        Ok(event) => {
            let relevant = event.tenant_id == tenant
                && (event.property_id.is_none() || query.property.is_none() || event.property_id == query.property);
            relevant
                .then(|| Ok(SseEvent::default().event("invalidate").json_data(&event.keys).expect("keys serialize")))
        }
        Err(BroadcastStreamRecvError::Lagged(_)) => Some(Ok(SseEvent::default().event("resync").data("{}"))),
    });
    Sse::new(ready.chain(updates)).keep_alive(KeepAlive::default())
}
```

Replace the whole file `crates/core-api/src/state.rs`:

```rust
use crate::graphql::{GqlSchema, build_schema};
use db::Event;
use sqlx::PgPool;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub schema: GqlSchema,
    /// Fan-out of database change events to this instance's SSE subscribers.
    pub events: broadcast::Sender<Event>,
    /// Production sets `Secure` cookies and disables GraphQL introspection.
    pub production: bool,
}

impl AppState {
    pub fn new(pool: PgPool, production: bool) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self { schema: build_schema(production), pool, events, production }
    }
}
```

Replace the whole file `crates/core-api/src/routes/mod.rs`:

```rust
pub(crate) mod auth;
mod health;
pub(crate) mod properties;

use crate::state::AppState;
use crate::{csrf, events, graphql, idempotency};
use axum::Router;
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post, put};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
pub use properties::CreatePropertyRequest;

pub fn router(state: AppState) -> Router {
    let commands = Router::new()
        .route("/api/v1/properties", post(properties::create))
        .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));

    let requests = Router::new()
        .route("/api/v1/auth/signup", post(auth::signup))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/me", get(auth::me))
        .route("/api/v1/session/tenant", put(auth::switch_tenant))
        .route("/graphql", post(graphql::handler))
        .merge(commands)
        .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(15)));

    // Long-lived, so kept outside the request timeout.
    let streams = Router::new().route("/api/v1/events", get(events::stream));

    Router::new()
        .merge(requests)
        .merge(streams)
        .layer(from_fn(csrf::require_csrf_header))
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
```

Replace the whole file `crates/core-api/src/main.rs`:

```rust
use anyhow::{Context, bail};
use core_api::config::Config;
use core_api::{AppState, events, router};
use sqlx::postgres::PgListener;
use tracing_subscriber::EnvFilter;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve().await,
        Some("migrate") => migrate().await,
        Some(other) => bail!("unknown command `{other}`; expected `serve` or `migrate`"),
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(config.production);
    let pool = db::connect(&config.database_url, config.database_max_connections).await?;
    let state = AppState::new(pool, config.production);
    let listener = PgListener::connect(&config.database_listen_url).await?;
    let mut listener = listener;
    listener.listen(db::CHANNEL).await?;
    events::spawn_listener(listener, state.events.clone());

    let tcp = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "listening");
    axum::serve(tcp, router(state)).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

/// Runs migrations as the schema owner (`DATABASE_OWNER_URL`), never as the API role.
async fn migrate() -> anyhow::Result<()> {
    init_tracing(false);
    let url = std::env::var("DATABASE_OWNER_URL").context("DATABASE_OWNER_URL is required")?;
    let pool = db::connect(&url, 1).await?;
    db::MIGRATOR.run(&pool).await?;
    tracing::info!("migrations applied");
    Ok(())
}

fn init_tracing(json: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tower_http=info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if json { builder.json().init() } else { builder.init() }
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("install Ctrl+C handler") };
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 16 tests. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(core-api): live cache invalidation over server-sent events"
```

### Task 12: OpenAPI document and schema export

The REST handlers already carry `#[utoipa::path]` annotations. This task collects them into one OpenAPI document and adds `export-schemas`, which writes `openapi.json` and `schema.graphql` for the frontend's type generation. A test pins the list of documented routes so a new route cannot be forgotten.

**Files:**
- Create: `crates/core-api/tests/openapi.rs`
- Modify: `crates/core-api/src/lib.rs`
- Create: `crates/core-api/src/openapi.rs`
- Create: `crates/core-api/src/bin/export-schemas.rs`

**Interfaces:**
- Produces: `core_api::openapi::ApiDoc` (`utoipa::OpenApi`).
- Produces binary: `cargo run -p core-api --bin export-schemas -- <dir>` writing `<dir>/openapi.json` and `<dir>/schema.graphql`.

- [ ] **Step 1: Write the failing test**

Create `crates/core-api/tests/openapi.rs`:

```rust
use core_api::openapi::ApiDoc;
use utoipa::OpenApi;

#[test]
fn the_openapi_document_lists_every_rest_route() {
    let doc = ApiDoc::openapi();
    let paths: Vec<&str> = doc.paths.paths.keys().map(String::as_str).collect();

    assert_eq!(
        paths,
        vec![
            "/api/v1/auth/login",
            "/api/v1/auth/logout",
            "/api/v1/auth/signup",
            "/api/v1/me",
            "/api/v1/properties",
            "/api/v1/session/tenant",
        ]
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
cargo test -p core-api --test openapi
```

Expected: FAIL: compile error, `core_api::openapi` not found.

- [ ] **Step 3: Implement**

Replace the whole file `crates/core-api/src/lib.rs`:

```rust
//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod auth;
pub mod config;
pub mod csrf;
pub mod error;
pub mod events;
pub mod graphql;
pub mod idempotency;
pub mod openapi;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
```

Create `crates/core-api/src/openapi.rs`:

```rust
use crate::routes::{CreatePropertyRequest, LoginRequest, SignupRequest, SwitchTenantRequest};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(title = "GoodFolk PMS API", version = "1"),
    paths(
        crate::routes::auth::signup,
        crate::routes::auth::login,
        crate::routes::auth::logout,
        crate::routes::auth::me,
        crate::routes::auth::switch_tenant,
        crate::routes::properties::create,
    ),
    components(schemas(
        SignupRequest,
        LoginRequest,
        SwitchTenantRequest,
        CreatePropertyRequest,
        identity::Profile,
        identity::TenantSummary,
        identity::Grant,
        identity::Role,
        property::Property,
    ))
)]
pub struct ApiDoc;
```

Create `crates/core-api/src/bin/export-schemas.rs`:

```rust
//! Writes `openapi.json` and `schema.graphql` into the given directory for frontend codegen.

use anyhow::Context;
use core_api::graphql::build_schema;
use core_api::openapi::ApiDoc;
use std::path::PathBuf;
use utoipa::OpenApi;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).context("usage: export-schemas <output-dir>")?);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("openapi.json"), ApiDoc::openapi().to_pretty_json()?)?;
    std::fs::write(dir.join("schema.graphql"), build_schema(false).sdl())?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: every test in the workspace; clippy and rustfmt clean. `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(core-api): OpenAPI document and schema export"
```

### Task 13: Frontend foundation: SPA, typed API clients, live cache invalidation

A SvelteKit single-page app (the staff app is behind a login, so it has no server rendering) served as static files. Vite proxies `/api` and `/graphql` to the API in development; production serves both from one origin. REST types come from the OpenAPI document and GraphQL types from the schema, both exported by the backend and committed. `connectEvents` turns server-sent events into TanStack Query invalidations.

**Files:**
- Create: `web/pms/` (scaffold), `web/pms/codegen.ts`, `web/pms/src/routes/+layout.ts`, `web/pms/src/lib/api/{problem,rest,graphql}.ts`, `web/pms/src/lib/{events,session,properties}.ts`, tests `web/pms/src/lib/api/problem.spec.ts`, `web/pms/src/lib/{events,session}.spec.ts`
- Modify: `web/pms/{vite.config.ts,eslint.config.js,package.json,.prettierignore}`, `web/pms/src/routes/+layout.svelte`
- Delete: `web/pms/src/routes/+page.svelte`, `web/pms/src/lib/index.ts`, `web/pms/src/lib/vitest-examples/`
- Generated (commit them): `web/pms/src/lib/api/{openapi.json,openapi.d.ts,schema.graphql,gql/}`

**Interfaces:**
- Consumes: the REST routes (Tasks 8–9), `query { properties { … } }` (Task 10), SSE events `ready` / `invalidate` / `resync` (Task 11), `export-schemas` (Task 12).
- Produces: `rest` (openapi-fetch client that always sends `x-goodfolk-csrf`), `idempotencyKey(): string`, `query(document, variables?, signal?)` (GraphQL, throws `ApiError`), `ApiError` / `toApiError(body, status)`, `applyEvent(client, type, data)` / `connectEvents(client, propertyId?) => disconnect`, `fetchMe(): Promise<Profile>`, `isTenantOwner(profile)`, `PropertiesDocument`, `propertiesKey = ['properties']`, `fetchProperties(signal?)`.

- [ ] **Step 1: Scaffold the app and add pinned dependencies**

```sh
bunx sv@0.17.1 create web/pms --template minimal --types ts \
  --add prettier eslint vitest="usages:unit" sveltekit-adapter="adapter:static" \
  --install bun --no-download-check
cd web/pms
bun add @tanstack/svelte-query@6.2.4 openapi-fetch@0.17.0
bun add -d openapi-typescript@7.13.0 @graphql-codegen/cli@7.4.2 @graphql-codegen/client-preset@6.2.0 graphql@17.0.2
rm -rf src/routes/+page.svelte src/lib/index.ts src/lib/vitest-examples
cd ../..
```

Expected: `web/pms/package.json` lists those exact versions. (Bun sometimes cannot resolve the `latest` tag of the GraphQL Codegen packages; exact versions avoid that.)

- [ ] **Step 2: Add the code-generation scripts**

In `web/pms/package.json`, add to `"scripts"`:

```json
"api:schemas": "cd ../.. && cargo run -q -p core-api --bin export-schemas -- web/pms/src/lib/api",
"codegen": "openapi-typescript src/lib/api/openapi.json -o src/lib/api/openapi.d.ts && graphql-codegen"
```

Append to `web/pms/.prettierignore` (generated files):

```gitignore
src/lib/api/gql/
src/lib/api/openapi.d.ts
src/lib/api/openapi.json
src/lib/api/schema.graphql
```

- [ ] **Step 3: Write the failing tests**

Create `web/pms/src/lib/api/problem.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { toApiError } from './problem';

describe('toApiError', () => {
	it('uses the problem detail as the message', () => {
		const error = toApiError(
			{ type: 'about:blank', title: 'Conflict', status: 409, detail: 'code taken' },
			409
		);

		expect(error.message).toBe('code taken');
		expect(error.status).toBe(409);
	});

	it('falls back to a generic problem for non-problem bodies', () => {
		const error = toApiError('<html>bad gateway</html>', 502);

		expect(error.message).toBe('Request failed');
		expect(error.status).toBe(502);
	});
});
```

Create `web/pms/src/lib/events.spec.ts`:

```ts
import { QueryClient } from '@tanstack/svelte-query';
import { describe, expect, it, vi } from 'vitest';
import { applyEvent } from './events';

describe('applyEvent', () => {
	it('invalidates exactly the keys named by an invalidate event', () => {
		const client = new QueryClient();
		const spy = vi.spyOn(client, 'invalidateQueries');

		applyEvent(client, 'invalidate', '["properties"]');

		expect(spy).toHaveBeenCalledTimes(1);
		expect(spy).toHaveBeenCalledWith({ queryKey: ['properties'] });
	});

	it('invalidates everything on resync', () => {
		const client = new QueryClient();
		const spy = vi.spyOn(client, 'invalidateQueries');

		applyEvent(client, 'resync', '{}');

		expect(spy).toHaveBeenCalledWith();
	});

	it('ignores unknown event types', () => {
		const client = new QueryClient();
		const spy = vi.spyOn(client, 'invalidateQueries');

		applyEvent(client, 'something-else', '[]');

		expect(spy).not.toHaveBeenCalled();
	});
});
```

Create `web/pms/src/lib/session.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { isTenantOwner, type Profile } from './session';

function profile(grants: Profile['grants']): Profile {
	return { user_id: 'u', email: 'a@b.lk', display_name: 'A', tenants: [], grants };
}

describe('isTenantOwner', () => {
	it('is true only for a tenant-wide owner grant', () => {
		expect(isTenantOwner(profile([{ role: 'owner' }]))).toBe(true);
		expect(isTenantOwner(profile([{ role: 'owner', property_id: 'p1' }]))).toBe(false);
		expect(isTenantOwner(profile([{ role: 'manager' }]))).toBe(false);
	});
});
```

- [ ] **Step 4: Run the tests to verify they fail**

```sh
cd web/pms && bun run test
```

Expected: FAIL: Vitest cannot resolve `./problem`, `./events` and `./session`.

- [ ] **Step 5: Implement**

Replace the whole file `web/pms/vite.config.ts`:

```ts
import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';

const api = process.env.GOODFOLK_API ?? 'http://localhost:8080';

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// SPA: every route falls back to index.html; the API is served from the same origin.
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	server: {
		proxy: {
			'/api': api,
			'/graphql': api
		}
	},
	test: {
		expect: { requireAssertions: true },
		projects: [
			{
				extends: './vite.config.ts',
				test: {
					name: 'server',
					environment: 'node',
					include: ['src/**/*.{test,spec}.{js,ts}'],
					exclude: ['src/**/*.svelte.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
```

Create `web/pms/codegen.ts`:

```ts
import type { CodegenConfig } from '@graphql-codegen/cli';

const config: CodegenConfig = {
	schema: 'src/lib/api/schema.graphql',
	// Queries live in .ts modules under src/lib, never inline in components.
	documents: ['src/lib/**/*.ts', '!src/lib/api/gql/**'],
	ignoreNoDocuments: true,
	generates: {
		'src/lib/api/gql/': {
			preset: 'client',
			config: {
				// Documents are plain strings: no GraphQL parser is shipped to the browser.
				documentMode: 'string',
				useTypeImports: true,
				scalars: { UUID: 'string' }
			}
		}
	}
};

export default config;
```

Replace the whole file `web/pms/eslint.config.js`:

```js
import prettier from 'eslint-config-prettier';
import path from 'node:path';
import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import { defineConfig, includeIgnoreFile } from 'eslint/config';
import globals from 'globals';
import ts from 'typescript-eslint';

const gitignorePath = path.resolve(import.meta.dirname, '.gitignore');

export default defineConfig(
	includeIgnoreFile(gitignorePath),
	// Generated by `bun run codegen`.
	{ ignores: ['src/lib/api/gql/**', 'src/lib/api/openapi.d.ts'] },
	js.configs.recommended,
	ts.configs.recommended,
	svelte.configs.recommended,
	prettier,
	svelte.configs.prettier,
	{
		languageOptions: { globals: { ...globals.browser, ...globals.node } },
		rules: {
			// typescript-eslint strongly recommend that you do not use the no-undef lint rule on TypeScript projects.
			// see: https://typescript-eslint.io/troubleshooting/faqs/eslint/#i-get-errors-from-the-no-undef-rule-about-global-variables-not-being-defined-even-though-there-are-no-typescript-errors
			'no-undef': 'off'
		}
	},
	{
		files: ['**/*.svelte', '**/*.svelte.ts', '**/*.svelte.js'],
		languageOptions: {
			parserOptions: {
				projectService: true,
				extraFileExtensions: ['.svelte'],
				parser: ts.parser
			}
		}
	},
	{
		// Override or add rule settings here, such as:
		// 'svelte/button-has-type': 'error'
		rules: {}
	}
);
```

Create `web/pms/src/routes/+layout.ts`:

```ts
// Staff app is behind a login: render in the browser only.
export const ssr = false;
```

Create `web/pms/src/routes/+layout.svelte`:

```svelte
<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import { ApiError } from '$lib/api/problem';

	let { children } = $props();

	const queryClient = new QueryClient({
		defaultOptions: {
			queries: {
				staleTime: 30_000,
				// Client errors (401, 403, 404…) will not succeed on retry.
				retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2
			}
		}
	});
</script>

<QueryClientProvider client={queryClient}>
	{@render children()}
</QueryClientProvider>
```

Create `web/pms/src/lib/api/problem.ts`:

```ts
/** RFC 9457 problem details, as returned by every API error. */
export interface Problem {
	type: string;
	title: string;
	status: number;
	detail?: string;
}

export class ApiError extends Error {
	constructor(readonly problem: Problem) {
		super(problem.detail ?? problem.title);
	}

	get status(): number {
		return this.problem.status;
	}
}

export function isProblem(value: unknown): value is Problem {
	return (
		typeof value === 'object' &&
		value !== null &&
		typeof (value as Problem).title === 'string' &&
		typeof (value as Problem).status === 'number'
	);
}

/** Turns any failed response body into an ApiError. */
export function toApiError(body: unknown, status: number): ApiError {
	return new ApiError(
		isProblem(body) ? body : { type: 'about:blank', title: 'Request failed', status }
	);
}
```

Create `web/pms/src/lib/api/rest.ts`:

```ts
import createClient from 'openapi-fetch';
import type { paths } from './openapi';

/** REST client for commands. Same origin, so the session cookie is sent automatically. */
export const rest = createClient<paths>({
	baseUrl: '',
	headers: { 'x-goodfolk-csrf': '1' }
});

/** A fresh key per user action; reuse it only when retrying that same action. */
export function idempotencyKey(): string {
	return crypto.randomUUID();
}
```

Create `web/pms/src/lib/api/graphql.ts`:

```ts
import type { TypedDocumentString } from './gql/graphql';
import { ApiError, toApiError } from './problem';

interface GraphQLResult<T> {
	data?: T;
	errors?: { message: string }[];
}

/** Runs a read-only GraphQL query. */
export async function query<TResult, TVariables>(
	document: TypedDocumentString<TResult, TVariables>,
	variables?: TVariables,
	signal?: AbortSignal
): Promise<TResult> {
	const response = await fetch('/graphql', {
		method: 'POST',
		headers: { 'content-type': 'application/json', 'x-goodfolk-csrf': '1' },
		body: JSON.stringify({ query: document.toString(), variables }),
		signal
	});
	const body: GraphQLResult<TResult> = await response.json();
	if (!response.ok) throw toApiError(body, response.status);
	if (body.errors?.length || !body.data) {
		throw new ApiError({
			type: 'about:blank',
			title: body.errors?.[0]?.message ?? 'Query failed',
			status: 200
		});
	}
	return body.data;
}
```

Create `web/pms/src/lib/events.ts`:

```ts
import type { QueryClient } from '@tanstack/svelte-query';

/**
 * Applies one server-sent event to the query cache. `invalidate` carries the cache keys that
 * changed; `resync` means events were missed, so everything is refetched.
 */
export function applyEvent(client: QueryClient, type: string, data: string): void {
	if (type === 'resync') {
		void client.invalidateQueries();
		return;
	}
	if (type === 'invalidate') {
		const keys: string[] = JSON.parse(data);
		for (const key of keys) void client.invalidateQueries({ queryKey: [key] });
	}
}

/** Keeps the cache fresh while the app is open. Returns a function that disconnects. */
export function connectEvents(client: QueryClient, propertyId?: string): () => void {
	const url = propertyId
		? `/api/v1/events?property=${encodeURIComponent(propertyId)}`
		: '/api/v1/events';
	const source = new EventSource(url);
	for (const type of ['invalidate', 'resync']) {
		source.addEventListener(type, (event) =>
			applyEvent(client, type, (event as MessageEvent).data)
		);
	}
	// EventSource reconnects by itself; anything missed while disconnected is refetched on reconnect.
	source.addEventListener('open', () => applyEvent(client, 'resync', '{}'));
	return () => source.close();
}
```

Create `web/pms/src/lib/session.ts`:

```ts
import { rest } from './api/rest';
import { toApiError } from './api/problem';
import type { components } from './api/openapi';

export type Profile = components['schemas']['Profile'];

export async function fetchMe(): Promise<Profile> {
	const { data, error, response } = await rest.GET('/api/v1/me');
	if (!response.ok || !data) throw toApiError(error, response.status);
	return data;
}

/** UI hint only; the API enforces permissions. */
export function isTenantOwner(profile: Profile): boolean {
	return profile.grants.some((grant) => grant.role === 'owner' && grant.property_id == null);
}
```

Create `web/pms/src/lib/properties.ts`:

```ts
import { graphql } from './api/gql';
import { query } from './api/graphql';

export const PropertiesDocument = graphql(`
	query Properties {
		properties {
			id
			code
			name
			timezone
			baseCurrency
		}
	}
`);

/** Query key shared with the server's `properties` invalidation event. */
export const propertiesKey = ['properties'] as const;

export async function fetchProperties(signal?: AbortSignal) {
	return (await query(PropertiesDocument, undefined, signal)).properties;
}
```

- [ ] **Step 6: Generate the API types, then run every check**

```sh
cd web/pms
bun run api:schemas
bun run codegen
bun run format
bun run lint && bun run check && bun run test && bun run build
```

Expected: `svelte-check` reports 0 errors and 0 warnings; ESLint and Prettier are clean; Vitest: `Tests  6 passed`; the build writes `build/index.html`.

- [ ] **Step 7: Commit**

```sh
cd ../..
git add -A
git commit -m "feat(web): SvelteKit SPA with typed REST and GraphQL clients and live cache invalidation"
```

### Task 14: Frontend screens: sign-up, sign-in, app shell, properties

Minimal, fast screens: plain CSS design tokens with light and dark themes, no animation library, no runtime CSS-in-JS. The app shell loads the profile once, sends visitors who are not signed in to `/login`, keeps one event stream open, and lists properties from GraphQL. Links and navigation use SvelteKit's typed `resolve()`, which the lint rules require.

**Files:**
- Create: `web/pms/src/app.css`, `web/pms/src/routes/(auth)/login/+page.svelte`, `web/pms/src/routes/(auth)/signup/+page.svelte`, `web/pms/src/routes/(app)/+layout.svelte`, `web/pms/src/routes/(app)/+page.svelte`, `web/pms/src/routes/(app)/properties/new/+page.svelte`, `web/pms/src/routes/(app)/p/[property]/+page.svelte`
- Modify: `web/pms/src/routes/+layout.svelte` (imports `app.css`)

**Interfaces:**
- Consumes: everything Task 13 produces.
- Produces routes: `/login`, `/signup`, `/` (property list), `/properties/new`, `/p/[property]`.

- [ ] **Step 1: Write the screens**

Create `web/pms/src/app.css`:

```css
:root {
	--bg: #ffffff;
	--surface: #f6f7f9;
	--border: #dfe3e8;
	--text: #1b1f24;
	--muted: #5b6570;
	--accent: #1f5fbf;
	--danger: #b42318;
	--radius: 6px;
	--space: 0.75rem;
	font-family:
		system-ui,
		-apple-system,
		'Segoe UI',
		Roboto,
		sans-serif;
	font-size: 15px;
	color: var(--text);
	background: var(--bg);
}

@media (prefers-color-scheme: dark) {
	:root {
		--bg: #111418;
		--surface: #1a1e24;
		--border: #2c323a;
		--text: #e6e9ed;
		--muted: #9aa4af;
		--accent: #6ea2ff;
		--danger: #ff7b72;
	}
}

* {
	box-sizing: border-box;
}

body {
	margin: 0;
}

input,
select,
button {
	font: inherit;
	color: inherit;
	padding: 0.45rem 0.6rem;
	border: 1px solid var(--border);
	border-radius: var(--radius);
	background: var(--bg);
}

button {
	cursor: pointer;
	background: var(--accent);
	border-color: var(--accent);
	color: #fff;
}

button:disabled {
	opacity: 0.6;
	cursor: default;
}

label {
	display: grid;
	gap: 0.25rem;
	color: var(--muted);
}

.form {
	display: grid;
	gap: var(--space);
	max-width: 24rem;
}

.error {
	color: var(--danger);
}
```

Replace the whole file `web/pms/src/routes/+layout.svelte`:

```svelte
<script lang="ts">
	import '../app.css';
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import { ApiError } from '$lib/api/problem';

	let { children } = $props();

	const queryClient = new QueryClient({
		defaultOptions: {
			queries: {
				staleTime: 30_000,
				// Client errors (401, 403, 404…) will not succeed on retry.
				retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2
			}
		}
	});
</script>

<QueryClientProvider client={queryClient}>
	{@render children()}
</QueryClientProvider>
```

Create `web/pms/src/routes/(auth)/login/+page.svelte`:

```svelte
<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { toApiError } from '$lib/api/problem';

	const client = useQueryClient();
	let email = $state('');
	let password = $state('');
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		const {
			data,
			error: problem,
			response
		} = await rest.POST('/api/v1/auth/login', {
			body: { email, password }
		});
		busy = false;
		if (!data) {
			error = toApiError(problem, response.status).message;
			return;
		}
		client.setQueryData(['me'], data);
		await goto(resolve('/'));
	}
</script>

<main class="form" style="margin: 10vh auto">
	<h1>Sign in</h1>
	<form class="form" onsubmit={submit}>
		<label>Email <input type="email" autocomplete="username" required bind:value={email} /></label>
		<label>
			Password
			<input type="password" autocomplete="current-password" required bind:value={password} />
		</label>
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<button disabled={busy}>Sign in</button>
	</form>
	<p><a href={resolve('/signup')}>Create an account</a></p>
</main>
```

Create `web/pms/src/routes/(auth)/signup/+page.svelte`:

```svelte
<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { toApiError } from '$lib/api/problem';

	const client = useQueryClient();
	let form = $state({ email: '', password: '', display_name: '', tenant_name: '' });
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		const {
			data,
			error: problem,
			response
		} = await rest.POST('/api/v1/auth/signup', {
			body: form
		});
		busy = false;
		if (!data) {
			error = toApiError(problem, response.status).message;
			return;
		}
		client.setQueryData(['me'], data);
		await goto(resolve('/'));
	}
</script>

<main class="form" style="margin: 10vh auto">
	<h1>Create your account</h1>
	<form class="form" onsubmit={submit}>
		<label>Your name <input required maxlength="200" bind:value={form.display_name} /></label>
		<label>
			Hotel or group name <input required maxlength="200" bind:value={form.tenant_name} />
		</label>
		<label
			>Email <input type="email" autocomplete="username" required bind:value={form.email} /></label
		>
		<label>
			Password (at least 12 characters)
			<input
				type="password"
				autocomplete="new-password"
				required
				minlength="12"
				maxlength="128"
				bind:value={form.password}
			/>
		</label>
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<button disabled={busy}>Create account</button>
	</form>
	<p><a href={resolve('/login')}>Already have an account? Sign in</a></p>
</main>
```

Create `web/pms/src/routes/(app)/+layout.svelte`:

```svelte
<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { ApiError } from '$lib/api/problem';
	import { connectEvents } from '$lib/events';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchMe } from '$lib/session';

	let { children } = $props();

	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal),
		enabled: !!me.data?.current_tenant
	}));

	$effect(() => {
		if (me.error instanceof ApiError && me.error.status === 401) void goto(resolve('/login'));
	});

	$effect(() => {
		if (me.data) return connectEvents(client);
	});

	async function switchTenant(event: Event) {
		const tenant_id = (event.currentTarget as HTMLSelectElement).value;
		const { data } = await rest.PUT('/api/v1/session/tenant', { body: { tenant_id } });
		if (data) {
			client.setQueryData(['me'], data);
			await client.invalidateQueries();
			await goto(resolve('/'));
		}
	}

	async function logout() {
		await rest.POST('/api/v1/auth/logout');
		client.clear();
		await goto(resolve('/login'));
	}
</script>

{#if me.data}
	<header>
		{#if me.data.tenants.length > 1}
			<select aria-label="Tenant" value={me.data.current_tenant} onchange={switchTenant}>
				{#each me.data.tenants as tenant (tenant.id)}
					<option value={tenant.id}>{tenant.name}</option>
				{/each}
			</select>
		{:else}
			<strong>{me.data.tenants[0]?.name}</strong>
		{/if}
		<nav aria-label="Properties">
			{#each properties.data ?? [] as property (property.id)}
				<a
					href={resolve('/(app)/p/[property]', { property: property.id })}
					aria-current={page.params.property === property.id ? 'page' : undefined}
				>
					{property.code}
				</a>
			{/each}
		</nav>
		<span class="spacer"></span>
		<span>{me.data.display_name}</span>
		<button onclick={logout}>Sign out</button>
	</header>
	<main>{@render children()}</main>
{/if}

<style>
	header {
		display: flex;
		align-items: center;
		gap: var(--space);
		padding: 0.5rem 1rem;
		border-bottom: 1px solid var(--border);
		background: var(--surface);
	}
	nav {
		display: flex;
		gap: 0.5rem;
	}
	nav a[aria-current='page'] {
		font-weight: 600;
	}
	.spacer {
		flex: 1;
	}
	main {
		padding: 1rem;
	}
</style>
```

Create `web/pms/src/routes/(app)/+page.svelte`:

```svelte
<script lang="ts">
	import { resolve } from '$app/paths';
	import { createQuery } from '@tanstack/svelte-query';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchMe, isTenantOwner } from '$lib/session';

	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
</script>

<h1>Properties</h1>
{#if properties.data?.length === 0}
	<p>No properties yet.</p>
{/if}
<ul>
	{#each properties.data ?? [] as property (property.id)}
		<li>
			<a href={resolve('/(app)/p/[property]', { property: property.id })}
				>{property.code} · {property.name}</a
			>
		</li>
	{/each}
</ul>
{#if me.data && isTenantOwner(me.data)}
	<p><a href={resolve('/properties/new')}>Add a property</a></p>
{/if}
```

Create `web/pms/src/routes/(app)/properties/new/+page.svelte`:

```svelte
<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { idempotencyKey, rest } from '$lib/api/rest';
	import { toApiError } from '$lib/api/problem';
	import { propertiesKey } from '$lib/properties';

	const client = useQueryClient();
	// One key per form: a double-click or network retry cannot create two properties.
	const key = idempotencyKey();
	let form = $state({ code: '', name: '', timezone: 'Asia/Colombo', base_currency: 'LKR' });
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		const {
			data,
			error: problem,
			response
		} = await rest.POST('/api/v1/properties', {
			body: {
				...form,
				code: form.code.toUpperCase(),
				base_currency: form.base_currency.toUpperCase()
			},
			params: { header: { 'Idempotency-Key': key } }
		});
		busy = false;
		if (!data) {
			error = toApiError(problem, response.status).message;
			return;
		}
		await client.invalidateQueries({ queryKey: propertiesKey });
		await goto(resolve('/(app)/p/[property]', { property: data.id }));
	}
</script>

<h1>Add a property</h1>
<form class="form" onsubmit={submit}>
	<label>
		Code (2–10 letters or digits)
		<input required pattern={'[A-Za-z0-9]{2,10}'} bind:value={form.code} />
	</label>
	<label>Name <input required maxlength="200" bind:value={form.name} /></label>
	<label>Time zone <input required bind:value={form.timezone} /></label>
	<label>
		Base currency
		<input required pattern={'[A-Za-z]{3}'} maxlength="3" bind:value={form.base_currency} />
	</label>
	{#if error}<p class="error" role="alert">{error}</p>{/if}
	<button disabled={busy}>Create property</button>
</form>
```

Create `web/pms/src/routes/(app)/p/[property]/+page.svelte`:

```svelte
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { fetchProperties, propertiesKey } from '$lib/properties';

	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const property = $derived(properties.data?.find((p) => p.id === page.params.property));
</script>

{#if property}
	<h1>{property.name}</h1>
	<dl>
		<dt>Code</dt>
		<dd>{property.code}</dd>
		<dt>Time zone</dt>
		<dd>{property.timezone}</dd>
		<dt>Base currency</dt>
		<dd>{property.baseCurrency}</dd>
	</dl>
{:else if properties.isSuccess}
	<p>Property not found.</p>
{/if}
```

- [ ] **Step 2: Run the checks**

```sh
cd web/pms
bun run lint && bun run check && bun run test && bun run build
```

Expected: all clean; `Tests  6 passed`.

- [ ] **Step 3: Check it in the browser**

In three terminals (with Postgres running and migrations applied as in Task 7):

```sh
DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk cargo run -p core-api
cd web/pms && bun run dev
```

Open http://localhost:5173 and check:
1. `/` redirects to `/login`. Choose "Create an account", sign up. You land on "Properties" with "No properties yet."
2. "Add a property": code `GAL`, name `Galle Fort Hotel`, time zone `Asia/Colombo`, currency `LKR`. You land on the property page, and `GAL` appears in the header.
3. Open a second tab on `/`. Create `KAN` in the first tab. The second tab's list and header update without a reload (live event).
4. Submitting `GAL` again shows "a property with this code already exists".
5. "Sign out" returns to `/login`, and `/` redirects there again.

Expected: all five behave as described. In DevTools → Network, `/api/v1/events` stays open and shows `ready`, then `invalidate ["properties"]` after each create.

- [ ] **Step 4: Commit**

```sh
cd ../..
git add -A
git commit -m "feat(web): sign-up, sign-in, app shell and property screens"
```

### Task 15: Container image, CI and developer README

A static musl build of `core-api` on distroless (no shell, non-root), ready for Cloud Run. CI runs the same checks as local development against a Postgres service container, plus cargo-deny (advisories, licences, sources) and a check that the committed frontend API types match the backend.

**Files:**
- Create: `README.md`, `Dockerfile`, `.dockerignore`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `core-api serve` / `migrate`, `deny.toml`, the web scripts from Task 13.
- Produces: image entrypoint `/core-api` (default command `serve`, `APP_ENV=production`, `PORT=8080`).

- [ ] **Step 1: Write the files**

Create `README.md`:

````markdown
# GoodFolk PMS

Multi-tenant, cloud-hosted hotel property management system.

- Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Delivery plan: [docs/ROADMAP.md](docs/ROADMAP.md)

## Layout

| Path | Contents |
|---|---|
| `crates/core-api` | axum HTTP API (REST commands, GraphQL reads, server-sent events) |
| `crates/db` | Postgres pool, migrations, tenant-scoped transactions, change events |
| `modules/*` | Domain modules (`identity`, `property`, …) |
| `migrations/` | SQL migrations, applied by `core-api migrate` |
| `web/pms` | SvelteKit staff app (single-page) |

## Development

Prerequisites: Rust (installed from `rust-toolchain.toml` by rustup), Bun 1.3+, and Docker or Podman with compose.

```sh
docker compose up -d postgres

export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
export DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk

cargo run -p core-api -- migrate   # as the schema owner
cargo run -p core-api              # API on http://localhost:8080, as the goodfolk_api role

cd web/pms && bun install && bun run dev   # app on http://localhost:5173
```

### Checks

```sh
DATABASE_URL=$DATABASE_OWNER_URL cargo test --workspace   # tests create throwaway databases
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check

cd web/pms
bun run api:schemas && bun run codegen   # after any API change; commit the result
bun run lint && bun run check && bun run test && bun run build
```
````

Create `Dockerfile`:

```dockerfile
# syntax=docker/dockerfile:1
# Static musl build of core-api on a distroless base (no shell, runs as non-root).
FROM rust:1.97-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p core-api --bin core-api && cp target/release/core-api /core-api

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /core-api /core-api
ENV APP_ENV=production PORT=8080
EXPOSE 8080
ENTRYPOINT ["/core-api"]
CMD ["serve"]
```

Create `.dockerignore`:

```gitignore
target
web
docs
.git
```

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  rust:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:17
        env:
          POSTGRES_USER: goodfolk_owner
          POSTGRES_PASSWORD: goodfolk_owner_dev
          POSTGRES_DB: goodfolk
        ports: ["5432:5432"]
        options: >-
          --health-cmd "pg_isready -U goodfolk_owner"
          --health-interval 2s --health-timeout 5s --health-retries 30
    env:
      # Tests create throwaway databases, so they connect as the owner (superuser).
      DATABASE_URL: postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
    steps:
      - uses: actions/checkout@v5
      - run: rustup show
      - uses: Swatinem/rust-cache@v2
      - name: Create database roles
        run: psql "$DATABASE_URL" -f deploy/dev/postgres-init.sql
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
      - uses: EmbarkStudios/cargo-deny-action@v2

  web:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: web/pms
    steps:
      - uses: actions/checkout@v5
      - run: rustup show
      - uses: Swatinem/rust-cache@v2
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
      - name: Generated API types match the backend
        run: |
          bun run api:schemas
          bun run codegen
          git diff --exit-code -- src/lib/api
      - run: bun run lint
      - run: bun run check
      - run: bun run test
      - run: bun run build
```

- [ ] **Step 2: Build and run the image against local Postgres**

```sh
docker build -t goodfolk-core-api .
docker run --rm --network host \
  -e DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk \
  -e PORT=18080 goodfolk-core-api &
sleep 2 && curl -si localhost:18080/readyz | head -1
```

Expected: the image builds; `HTTP/1.1 204 No Content`; `docker images goodfolk-core-api` shows a small image (tens of MB). Stop the container afterwards.

- [ ] **Step 3: Run cargo-deny locally**

```sh
cargo install cargo-deny --locked
cargo deny check
```

Expected: `advisories ok, bans ok, licenses ok, sources ok`.

- [ ] **Step 4: Commit and push, then confirm CI**

```sh
git add -A
git commit -m "ci: container image, CI workflow and developer README"
git push -u origin HEAD
```

Expected: both CI jobs (`rust`, `web`) pass on the pushed branch.

## Phase 0 done when

- A user signs up, creates two properties, and both appear in the header and list.
- Signing up a second user in another browser shows an empty list. Tenant isolation holds over HTTP (`tenants_never_see_each_others_properties`) and in the database (`crates/db/tests/isolation.rs`).
- A change in one tab appears in another without reloading.
- CI is green: fmt, clippy, 40 Rust tests, cargo-deny, API types up to date, lint, svelte-check, 6 web tests, build.
