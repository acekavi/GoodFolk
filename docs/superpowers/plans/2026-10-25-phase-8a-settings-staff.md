# Phase 8a Staff, Invitations and Settings — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Owners and managers invite staff by email (AWS SES), manage their roles and revoke them. A permissions matrix
and a POS role are added. An advisory deposit rule per rate plan shows what is due. Every property setting this phase
owns is editable from one Settings area.

**Architecture:**
- **`modules/identity`** gains invitations, members and the permission matrix.
- **A new `crates/mail`** sends email: SES v2 in production, a dev outbox directory locally. jobs-svc sends it from a
  new `/mail` push endpoint, fed by the outbox topic `mail-send`.
- **`rates` and `reservations`** gain the deposit rule and its snapshot. `audit`'s review gains the outstanding-deposit
  list.
- **core-api** wires REST, GraphQL and events.
- **The SPA** gets:
  - the invite page;
  - Staff and Permissions;
  - the Settings index with General and Block reasons;
  - the SLTDA licence field;
  - the deposit rule editor and its displays.

**Tech Stack:** Rust 1.99, axum 0.8, sqlx 0.9, async-graphql 7.2, PostgreSQL 17 (forced RLS), SvelteKit 2 / Svelte 5,
Playwright, Bun. For email, `aws-sdk-sesv2` or `aws-sigv4` with `reqwest`; the builder chooses, and cargo-deny must
pass.

**Spec:** [docs/specs/phase-8a-settings-staff.md](../../specs/phase-8a-settings-staff.md). It is the authority.

**Execution method (the owner's standing preference):**
- Each task is implemented once on branch `phase-8a-settings-staff`, then reviewed, with fix rounds.
- A final whole-branch review and one fix wave follow, then a manual testing guide.

## Global Constraints

- **Environment:**
  - Postgres on 5432. Tests use `DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`.
  - RustFS on :9000.
  - The e2e DB is `goodfolk_e2e_8a`; the controller creates and migrates it.
  - Never drop a database. Never `pkill -f`; stop only exact PIDs you started. Leave the owner's vite dev server
    alone.
  - Builds use `CARGO_BUILD_JOBS=3` and `cargo test -j 3`.
- **Green before every commit:**
  - **Rust:** `cargo test -j 3 -p <each crate the task changes>`. For core-api, run only the test binaries the task
    adds or changes. Then `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`. There
    is no workspace run per task (the owner, 2026-10-25).
  - **Web:** `bun run lint && bun run check && bun run test && bun run build`, plus the default e2e suite when a task
    touches shared UI.
  - After an API change, run `bun run api:schemas && bun run codegen` and commit the generated files.
  - Use Bun only.
- **Commits:** the exact messages given. No body, no trailer, no AI attribution. Never push.
- **Migrations** start at `0035`. Every `security definer` function pins
  `search_path = pg_catalog, public, pg_temp`.
- **Names (exact):**
  - Role: `pos`.
  - Permission: `StaffManage` (owner, manager).
  - Junior roles: `front_desk`, `housekeeping`, `hk_supervisor`, `accountant`, `pos`.
  - Invitation statuses: `pending`, `accepted`, `revoked`, `expired`.
  - Outbox topic: `mail-send`.
  - Event: `staff:<tenant>`.
  - Deposit `kind`: `percent`, `nights`, `amount`. Deposit `due`: `at_booking`, `days_before_arrival`.
- **Tokens:**
  - 32 random bytes, base64url, shown only in the link.
  - Stored as SHA-256.
  - Expire in 7 days.
  - Compared by hash lookup. An unknown, expired, revoked or used token gives the same `410` "This invitation is no
    longer valid".
  - A token never appears in a log line, an audit row or an error.
- **Email:**
  - `MAIL_FROM`, `AWS_REGION` and the AWS credentials are needed in production.
  - `MAIL_OUTBOX_DIR` (default `target/mail-outbox`) is used in development.
  - `APP_BASE_URL` builds the links.
  - `APP_ENV=production` refuses the dev mailer.
- **Rules for managers:**
  - A manager acts only on members whose every grant is junior.
  - They give only junior grants, at properties where they hold `manager` (any property, with a tenant-wide `manager`
    grant).
  - A refusal is `403`. Only owners grant `manager`. Nobody grants `owner`.

## Review Focus

- **A token can't be replayed, probed or leaked.** A second accept, an expired or resent link, and an unknown token
  all answer the same `410`. The token appears in no log, audit row or response. Test: Task 3.
- **Removing someone ends their access now.** Their open tab's next request is `401`, and a member of another tenant is
  untouched. Test: Tasks 4 and 11.
- **A manager can't climb.** A manager can't invite or re-grant a manager, act on a property they don't manage, or
  touch a member who holds a manager grant. The last owner can't be removed or demoted. Test: Tasks 3 and 4.
- **The POS role is narrow.** A `pos` user can post a bill and read outlets and in-house rooms, and every other route
  group gives `403`. Test: Task 5.
- **A deposit rule never blocks and never rewrites history.** Bookings, check-in and the close carry on with deposits
  outstanding, and a rule edit leaves existing bookings' terms alone. Test: Task 6.

---

### Task 1: Schema and permissions

**Files:**
- Create `migrations/0035_staff_settings.sql`.
- Modify `modules/identity/src/rbac.rs` and the web `session.ts` ACTIONS.
- Tests: `crates/db/tests/isolation.rs` and `crates/db/tests/staff_schema.rs` (new).

**Migration:**
```sql
alter table role_grant drop constraint role_grant_role_check;   -- check its real name in 0013
alter table role_grant add constraint role_grant_role_check
  check (role in ('owner', 'manager', 'front_desk', 'housekeeping', 'accountant', 'hk_supervisor', 'pos'));
alter table membership add column version integer not null default 1;

create table invitation (
  id uuid primary key,
  tenant_id uuid not null references tenant (id) on delete cascade,
  email citext not null,
  display_name text not null check (length(display_name) between 1 and 200),
  grants jsonb not null check (jsonb_typeof(grants) = 'array' and jsonb_array_length(grants) between 1 and 50),
  token_hash bytea not null unique,
  status text not null check (status in ('pending', 'accepted', 'revoked', 'expired')),
  invited_by uuid references app_user (id) on delete set null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null,
  accepted_at timestamptz, accepted_user_id uuid references app_user (id) on delete set null,
  check ((status = 'accepted') = (accepted_at is not null))
);
create unique index invitation_one_pending_idx on invitation (tenant_id, email) where status = 'pending';

alter table property add column sltda_licence text check (length(sltda_licence) between 1 and 50);
alter table rate_plan add column deposit_rule jsonb check (jsonb_typeof(deposit_rule) = 'object');
alter table reservation_room add column deposit_terms jsonb check (jsonb_typeof(deposit_terms) = 'object');
```

- **RLS:** forced, with the tenant policy, on `invitation`.
- **Token lookups across tenants:** preview and accept find an invitation by token before any tenant is known. Give
  this to two definers that the app role may execute:
  - `app.invitation_by_token(hash bytea)` returns `(id, tenant_id, email, display_name, grants, status, expires_at,
    tenant_name, inviter_name)`;
  - `app.accept_invitation_row(id uuid, user_id uuid)` sets the invitation `accepted` only if it is `pending` and not
    expired, and returns whether it did.

  Membership and grants are then inserted in a tenant-scoped transaction. Add an owner read/update policy as 0022 does.
- **Grants:** `invitation` gets select, insert and update (no delete).
- **Rust:**
  - `Role::Pos` (`"pos"`) with `PosPost` only;
  - `Permission::StaffManage` ("Allows: inviting staff, changing their roles and removing them; managers only for
    junior roles at their properties") for Owner and Manager;
  - `pub const JUNIOR_ROLES: [Role; 5]`;
  - a `pub fn describe(permission) -> &'static str`, with each permission's "Allows" text moved from its doc comment
    into a match so the matrix can serve it;
  - `pub const ALL_PERMISSIONS` and `ALL_ROLES` arrays.

- [ ] **Tests:**
  - an isolation case for `invitation`;
  - **schema:**
    - the `pos` role is accepted;
    - a second pending invitation for the same tenant and email is refused;
    - the app role cannot delete an invitation;
    - each definer works across tenants and pins `search_path`;
  - **RBAC:** the matrix for `Pos` and `StaffManage`; every permission has a non-empty `describe`.
- [ ] Commit with the message `feat(db): invitations, a POS role, membership versions, deposit rules and the SLTDA licence`.

### Task 2: Mail

**Files:**
- Create `crates/mail/{Cargo.toml,src/lib.rs,src/ses.rs,src/dev.rs}`.
- Create `crates/jobs-svc/src/mail.rs`, and modify jobs-svc's `lib.rs`, `config.rs` and `main.rs`.
- Modify core-api `main.rs`: add `mail-send` to its bus topics.
- Tests: `crates/mail/tests/*.rs`, `crates/jobs-svc/tests/mail.rs`.

**Interfaces (exact):**
```rust
pub struct Message { pub to: String, pub subject: String, pub text: String, pub html: String }
#[async_trait] pub trait Mailer: Send + Sync { async fn send(&self, message: &Message) -> Result<(), MailError>; }
#[derive(Debug, thiserror::Error)] pub enum MailError { #[error("{0}")] Retryable(String), #[error("{0}")] Permanent(String) }
pub struct SesMailer { /* region, from, credentials, client */ }   // SES v2 SendEmail
pub struct DevMailer { pub dir: PathBuf }                           // writes <uuid>.json {to, subject, text, html}
pub fn from_env(production: bool) -> anyhow::Result<Arc<dyn Mailer>>;
/// The `mail-send` payload the outbox carries.
#[derive(Serialize, Deserialize)] pub enum MailJob { Invitation { invitation_id: Uuid, tenant_id: Uuid, token: String } }
```

**Rules:**
- **jobs-svc `POST /mail`** sits behind svc-auth, like media-svc's `/render`. It decodes a `MailJob::Invitation`, then
  reads the invitation, tenant, inviter and property names through `app.invitation_by_id` (add this definer in this
  task's migration `0036` if Task 1 didn't). It renders the email from the spec's text, sends it, and answers:
  - `204` on success;
  - `503` on `Retryable`, so the bus retries;
  - `204` with an error log on `Permanent`, since there is no point retrying;
  - `204` and nothing sent when the invitation is no longer pending.
- **The token travels in the outbox payload.** It must, to build the link. This is the one place it is stored, and the
  outbox purge already deletes delivered rows. Document this in the crate docs.
- **Logs:** never log the payload; log the recipient and subject only.
- **SES:** `SendEmail` with `Content.Simple` (subject, text, html) from `MAIL_FROM`. A throttling or 5xx response is
  `Retryable`; other 4xx responses are `Permanent`.
- **Choice of client:** compare `aws-sdk-sesv2` (with only its rustls feature) against `aws-sigv4` + `reqwest`, by
  dependency count and cargo-deny result. Pick the lighter one that passes, and record why in the crate docs.

- [ ] **Tests:**
  - the dev mailer writes the JSON file;
  - the SigV4 signing matches AWS's published test vector, or the SDK's own request is checked against a local mock
    server that inspects the `Authorization` header shape;
  - jobs-svc `/mail` refuses calls without auth;
  - a pending invitation sends one message to the dev outbox with the link, and a revoked one sends nothing;
  - the log captured during a send contains no token.
- [ ] Commit with the message `feat(mail): invitation emails through SES, or a local outbox in development`.

### Task 3: Invitations

**Files:** create `modules/identity/src/invitations.rs`, and modify `account.rs` (sign-in with no memberships). Test:
`modules/identity/tests/invitations.rs`.

**Interfaces (exact):**
```rust
pub struct GrantSpec { pub role: Role, pub property_id: Option<Uuid> }
pub struct Invitation { pub id: Uuid, pub email: String, pub display_name: String, pub grants: Vec<GrantSpec>,
    pub status: InvitationStatus, pub invited_by: Option<Uuid>, pub created_at: OffsetDateTime, pub expires_at: OffsetDateTime }
pub struct Invited { pub invitation: Invitation, pub token: String }   // token only returned to the mail job
/// actor's grants decide what they may give (the manager rules). Writes the outbox row `mail-send`.
pub async fn invite(tx: &mut Tx, tenant: TenantId, actor: UserId, actor_grants: &[Grant], email: &str, display_name: &str, grants: &[GrantSpec]) -> Result<Invitation, StaffError>;
pub async fn resend(tx, tenant, actor, actor_grants, id: Uuid) -> Result<Invitation, StaffError>;
pub async fn revoke_invitation(tx, tenant, actor, actor_grants, id: Uuid) -> Result<Invitation, StaffError>;
pub async fn list_invitations(tx, status: Option<InvitationStatus>) -> Result<Vec<Invitation>, StaffError>;
pub struct Preview { pub tenant_name: String, pub inviter: Option<String>, pub email: String, pub grants: Vec<(Role, Option<String>)>, pub existing_account: bool }
pub async fn preview(pool: &PgPool, token: &str) -> Result<Preview, StaffError>;        // Gone on anything invalid
pub enum Acceptor { NewUser { password: String }, SignedIn(UserId) }
pub async fn accept(pool: &PgPool, token: &str, acceptor: Acceptor) -> Result<(UserId, TenantId), StaffError>;
#[derive(Debug, thiserror::Error)] pub enum StaffError { NotFound(&'static str), Gone, Forbidden(String), Conflict(String), Invalid(String), VersionMismatch, Database(#[from] sqlx::Error) }
```

**Rules:**
- **Validation:**
  - the email is valid and lowercased (`citext`);
  - the display name is 1–200 characters;
  - there are 1–50 grants, with the properties in the tenant;
  - `owner` is refused (`Invalid`);
  - the manager rules from the Global Constraints apply (`Forbidden`, naming the role or property);
  - an email that is already a member is `Conflict`, and so is a second pending invite for it.
- **Tokens:** generated with `rand::rngs::OsRng`, 32 bytes, base64url without padding. The hash is SHA-256.
- **Writes:**
  - `invite` and `resend` write the `mail-send` outbox row with `MailJob::Invitation`, through the events crate's
    outbox insert, in the same transaction.
  - `resend` replaces `token_hash` and `expires_at`.
  - A pending invitation past `expires_at` is treated as expired when read; set `expired` lazily.
  - Every write audits (with no token) and notifies `staff:<tenant>`.
- **Accept:**
  - **A new user:** the email must have no account (otherwise `Conflict` "Sign in as <email> to accept"). The password
    follows sign-up's rules, and the account is created as sign-up does.
  - **`SignedIn(user)`:** the user's email must equal the invitation's (`Forbidden`).
  - **Then:** membership with `version` 1, grants, mark accepted through the definer (exactly once under concurrency),
    audit `invitation.accepted`.
  - Return `(user, tenant)`; the route starts or switches the session.
- **Sign-in with no memberships:** `authenticate` and the session work with no active tenant. The profile lists no
  tenants. Check the existing behaviour and fix it if it fails.

- [ ] **Tests:**
  - **Tokens:**
    - accept works once, and a second accept is `Gone`;
    - expired is `Gone`;
    - after a resend the old token is `Gone` and the new one works;
    - a revoked invitation is `Gone`;
    - an unknown token is `Gone`;
    - concurrent accepts: exactly one succeeds.
  - **Accepting:**
    - a new user gets exactly the invited grants;
    - an existing user must be signed in as that email;
  - **The manager rules:**
    - a manager inviting a `manager` is `Forbidden`;
    - a manager inviting at a property they don't manage is `Forbidden`;
    - a tenant-wide junior grant from a per-property manager is `Forbidden`;
    - the same invitations from an owner succeed.
  - **Outbox and audit:**
    - the outbox row exists in the same transaction (rolled back with it);
    - no audit row contains the token.
  - A user with no memberships can sign in.
- [ ] Commit with the message `feat(identity): staff invitations with single-use, expiring tokens`.

### Task 4: Members, grants, revocation and the matrix

**Files:** create `modules/identity/src/members.rs`. Test: `modules/identity/tests/members.rs`.

**Interfaces (exact):**
```rust
pub struct Member { pub user_id: Uuid, pub email: String, pub display_name: String, pub grants: Vec<GrantSpec>, pub joined_at: OffsetDateTime, pub version: i32 }
pub async fn members(tx: &mut Tx) -> Result<Vec<Member>, StaffError>;
pub async fn set_grants(tx, tenant, actor, actor_grants, user: Uuid, expected_version: i32, grants: &[GrantSpec]) -> Result<Member, StaffError>;
/// Deletes the membership (grants cascade) and the user's sessions whose active_tenant_id is this tenant.
pub async fn revoke_member(tx, tenant, actor, actor_grants, user: Uuid) -> Result<(), StaffError>;
pub struct MatrixRow { pub permission: &'static str, pub allows: &'static str, pub roles: Vec<Role> }
pub fn permission_matrix() -> Vec<MatrixRow>;
```

**Rules:**
- **Order of checks:** lock the membership row (`for update`), check the version, then the rules.
- **Nobody acts on themself** (`Forbidden`).
- **The last owner:** an `owner` grant can't be removed from the tenant's last owner, and the last owner can't be
  revoked (`Conflict`).
- **Managers:** the manager rules apply to both the target's current grants and the new ones.
- **Grants:** set by replacement in the transaction, then the version bumps. Every other membership change bumps it too.
- **Revocation:** delete from `session where user_id = $1 and active_tenant_id = $tenant`. This needs a definer,
  `app.end_sessions(user, tenant)`, because `session` has no RLS and the app role may lack delete; check and add it.
- **Audit:** `membership.grants_changed` with previous and new grants, and `membership.revoked`. Both notify
  `staff:<tenant>`.

- [ ] **Tests:**
  - set grants: the version moves, and a stale version is `VersionMismatch`;
  - the last owner can't be demoted or removed;
  - nobody can act on themself;
  - a manager acting on a member with a manager grant is `Forbidden`;
  - revoke deletes that tenant's sessions and keeps the user's sessions for another tenant;
  - the matrix lists every permission with its roles, matching `Role::permits`.
- [ ] Commit with the message `feat(identity): members, grants, revocation and the permissions matrix`.

### Task 5: The POS role's reads

**Files:**
- Modify `crates/core-api/src/graphql.rs`: add `posInHouse(propertyId)` and an `outlets` gate that accepts `PosPost`.
- Test: `crates/core-api/tests/pos_role.rs` (new).

**Rules:**
- `posInHouse` returns `{reservationRoomId, roomNumber, guestName}` for checked-in in-house stays, using
  `folio::in_house_stays` plus a join. It needs `PosPost`.
- The outlets read accepts `PosPost` as well as its current permission.
- **Test:** a `pos` user (grant via `add-staff` or SQL) can:
  - post a POS bill;
  - read `posInHouse` and outlets;
  - and gets `403` (or a GraphQL permission error) on one representative route or field from every other group
    (reservations, folio, rates, rooms, housekeeping, linen, laundry, night audit, settings, staff).
- [ ] Commit with the message `feat(core-api): a POS role that posts bills and sees what a till needs`.

### Task 6: The advisory deposit rule

**Files:**
- Modify `modules/rates/src/plans.rs` (`deposit_rule` on create and update, validated).
- Modify `modules/reservations/src/reservations.rs` (snapshot to `reservation_room.deposit_terms` at booking).
- Create `modules/reservations/src/deposit.rs` (`deposit_due`).
- Modify `modules/audit/src/review.rs` (the `deposits_outstanding` list).
- Tests in each crate.

**Interfaces (exact):**
```rust
// rates
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")] pub enum DepositKind { Percent, Nights, Amount }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")] pub enum DepositDue { AtBooking, DaysBeforeArrival { days: i32 } }  // 0–365
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DepositRule { pub kind: DepositKind, pub value: i64, pub due: DepositDue }   // percent: 1–10000 bp; nights: 1–30; amount: > 0
// reservations
pub struct DepositDueLine { pub currency: String, pub amount: i64, pub due_date: Date, pub paid: i64, pub outstanding: i64 }
pub async fn deposit_due(tx: &mut Tx, reservation: Uuid) -> Result<Vec<DepositDueLine>, ReservationsError>;  // per currency
// audit review
pub struct DepositRow { pub reservation_id: Uuid, pub confirmation: String, pub guest: String, pub arrival: Date, pub currency: String, pub due: i64, pub paid: i64, pub due_date: Date }
```

**Rules:**
- **Each room's due amount** is computed from its own `deposit_terms` and nights, using the same `penalty_amount`
  arithmetic shapes (percent of the total half-up, first N nights' room amounts, or the amount), capped at the room's
  total.
- **Totals:** rooms are summed per currency. The due date is the earliest of the rooms' due dates (the booking's
  `booked_business_date`, or arrival − N).
- **Paid** is the deposit folio's payments in that currency, net of refunds, with `transfer` excluded.
- **Cancelled and no-show rooms** count for nothing.
- **Outstanding** is `max(0, due − paid)`.
- **Deposits outstanding**, the review's list for D, contains reservations with a due date ≤ D, an outstanding amount
  above 0 and at least one room still confirmed or tentative. It is ordered by arrival and is not in `blocking`.
- **Rate plans:** a derived plan has its own rule, with no inheritance.

- [ ] **Tests:**
  - each kind, with the cap;
  - both due types;
  - two rooms in two currencies;
  - a payment, then a refund, moving the outstanding amount;
  - a rule edit after booking leaves `deposit_terms` alone;
  - booking and check-in are never refused for an outstanding deposit;
  - the review list includes and excludes rows correctly and doesn't change `blocking`.
- [ ] Commit with the message `feat: an advisory deposit rule per rate plan, owed per reservation and listed at night audit`.

### Task 7: Property name and the SLTDA licence

**Files:**
- Modify `modules/property/src/lib.rs`: `name` is already in `PropertyChanges`, so check it. Tax settings gain
  `sltda_licence`.
- Modify `modules/folio/src/invoices.rs`, `crates/media-types` (`Supplier.sltda_licence: Option<String>`) and
  `crates/media-svc/templates/invoice.typ` (print it under the TIN when set).
- Tests: property, folio invoices and the media-svc render test.

- [ ] **Tests:**
  - the property name round-trips;
  - `sltda_licence` round-trips and is validated to 1–50 characters with surrounding whitespace refused;
  - an issued invoice's snapshot carries it;
  - the render test's PDF text includes it.
- [ ] Commit with the message `feat: the property name and an SLTDA licence on invoices`.

### Task 8: core-api

**Files:**
- Create `crates/core-api/src/routes/staff.rs`.
- Modify `routes/{mod,auth,properties,rates,taxes}.rs`, `graphql.rs`, `events.rs`, `error.rs` and `openapi.rs`.
- Test: `crates/core-api/tests/staff.rs` (new), plus the touched tests.

**REST** (exact):
- **Staff management** (`StaffManage`, tenant scope):
  - `POST /api/v1/invitations` (idempotent);
  - `POST /api/v1/invitations/{id}/resend`;
  - `POST /api/v1/invitations/{id}/revoke`;
  - `PUT /api/v1/members/{user}/grants` (`If-Match`, `ETag`);
  - `DELETE /api/v1/members/{user}`.
- **Public invitation routes:** `POST /api/v1/invitations/preview` and `POST /api/v1/invitations/accept`.
  - Both sit outside CSRF like sign-up, and go through the sign-in throttle keyed by the token hash.
  - Accept takes `{token, password?}`. With a valid session cookie it accepts as that user and switches the session.
    Without one, it creates the user and session cookie.
- **Other routes:** the property `PATCH` takes `name`; the tax settings `PATCH` takes `sltda_licence`; rate plan
  create and `PATCH` take `deposit_rule`.

**GraphQL:**
- `members` and `invitations(status)` need `StaffManage`.
- `permissionMatrix` is open to any member.
- `posInHouse` comes from Task 5.
- `Reservation.depositDue` needs `FolioView` or `ReservationsView`; take the existing reservation gate.
- The `nightAudit.depositsOutstanding` field.
- `PropertyNode.name` is editable, and `sltdaLicence` goes on the tax settings node.

**Events:** `staff:<tenant>` reaches only members holding `StaffManage`.

**Errors:** `StaffError::Gone` maps to `410`, with no detail beyond the fixed text.

- [ ] **Tests:**
  - **The invitation flow over HTTP:**
    - invite, read the dev outbox file, preview, accept as a new user with a session cookie set, then `GET /me` shows
      the grants;
    - a second accept is `410`;
    - an existing user accepts while signed in.
  - **Revocation:** revoke, then the member's old cookie gives `401` on `/me`.
  - **Managers:** 403s for a manager inviting a manager, and for anything a front desk user tries.
  - **The token:** it is absent from responses (except none: the API never returns it) and from captured logs.
  - **OpenAPI:** the paths and `ETag`s are listed.
- [ ] Regenerate the client types: `bun run api:schemas && bun run codegen`.
- [ ] Commit with the message `feat(core-api): staff invitations, members, the matrix and deposits`.

### Task 9: The invitation page in the SPA

**Files:** create `web/pms/src/routes/(auth)/invite/[token]/+page.svelte`, with `lib/staff.ts` helpers as needed. Make
sure the signed-out layout allows it.

**Behaviour:**
- **Preview:** shows the tenant, inviter, email and roles, or "This invitation is no longer valid".
- **A new account:** asks for a password (sign-up rules) and **Accept**, then lands on the first granted property or
  the property list.
- **An existing account:** asks the person to sign in as that email (a sign-in link that returns to this page), then
  **Accept**.
- **Errors:** a 409 or 403 shows the server's message.

- [ ] **Unit tests** for the helpers. Run lint, check, test and build.
- [ ] Commit with the message `feat(web): accept an invitation`.

### Task 10: Staff, Permissions and the Settings index

**Files:**
- Create the tenant-level routes `(app)/settings/{+page.svelte,staff,permissions}`.
- Create the property-level routes `(app)/p/[property]/settings/{+page.svelte (index),general,block-reasons}`.
- Modify the taxes page (the SLTDA field) and the rate plan editor (the deposit rule).
- Modify the new-reservation review step and `RoomCard` / `ReservationModal` (deposit due).
- Modify the night-audit page (a sixth section).
- Modify `settings.ts` and `session.ts`, the nav and the layout.

**Behaviour:**
- **Staff page:**
  - members and pending invitations;
  - an **Invite** dialog with email, name and grant rows (role, and property or "All properties"), filtered to what the
    user may give;
  - **Change roles** (`If-Match`, version snapshot at open);
  - **Remove** (confirm), with **Resend** and **Revoke** on invitations;
  - actions hidden by the manager rules;
  - live through `staff:` events.
- **Permissions:** the matrix as a table, with roles as columns.
- **Settings index:** groups and links exactly as the spec lists them, each shown only with its permission.
- **General:** name, check-in, check-out and audit times (`If-Match`), with the time zone and base currency read-only.
- **Block reasons:** list, add, rename and deactivate.
- **The deposit rule editor** sits beside the cancellation policy on the rate plan form: none, percent, nights or
  amount; at booking or N days before arrival.
- **Deposit displays:**
  - the new-reservation review shows "Deposit due <amount> by <date>";
  - the reservation modal shows due, paid and outstanding;
  - the night audit's **Deposits outstanding** section is informational, outside the blocking count.

- [ ] **Unit tests:** grant-row filtering by manager rules, matrix building, deposit text. Run lint, check, test and
  build, then the default e2e suite once (shared screens).
- [ ] Commit with the message `feat(web): staff, permissions, the settings index, general settings, block reasons and deposits`.

### Task 11: End-to-end tests

**Files:**
- Create `web/pms/tests/e2e/staff.spec.ts` and `deposits.spec.ts`.
- Modify `playwright.config.ts`: set `MAIL_OUTBOX_DIR` for jobs-svc to a temp directory the tests read, set
  `APP_BASE_URL`, and add `PUSH_ENDPOINT_MAIL_SEND` for core-api.

- [ ] **Invitation:**
  - an owner invites a front-desk clerk;
  - the test reads the newest message in the outbox dir, follows the link, sets a password, and lands in the property
    with front-desk navigation.
- [ ] **Revocation:**
  - the owner removes them;
  - the clerk's open page is signed out on its next request.
- [ ] **A manager:** they can invite housekeeping, but not a manager (the option is absent, and the server answers
  403 to a forced request).
- [ ] **Deposits:**
  - a plan with a 50% at-booking rule;
  - a booking shows the deposit due;
  - the night audit lists it;
  - taking a payment on the deposit folio clears it.
- [ ] Run the default suite twice.
- [ ] Commit with the message `test(e2e): invitations, revocation, manager limits and deposits`.

### Task 12: Documentation

- [ ] **README:**
  - "Email" (SES set-up, the dev outbox, `APP_BASE_URL`);
  - "Trying staff and settings by hand", in the style of the earlier walkthroughs;
  - the env tables updated;
  - `add-staff` documented as a dev and seeding tool.
- [ ] **ROADMAP:**
  - Phase 8a marked done, with deviations (the ledger's rulings and the spec's Builder's calls);
  - 8b (Channex) left as next, once a sandbox exists;
  - the Redis decision recorded (deferred until real users).
- [ ] **Design docs:**
  - ARCHITECTURE §7.7 and §5 (mail through the outbox);
  - `docs/design/api-conventions.md`: `StaffManage`, `pos`, `staff:` events, the public invitation routes, `410`;
  - `docs/design/data-model.md`.
- [ ] Commit with the message `docs: staff, invitations, email and settings`.
