# Phase 8a: Staff, Invitations and Settings (Spec)

**Depends on:** Phases 0–7b and laundry. **Implementation plan:** written at phase start in `docs/superpowers/plans/`.
Design: [ARCHITECTURE.md §7.7](../ARCHITECTURE.md).

Phase 8 is split (the owner, 2026-10-25):
- **8a** (this spec): staff invitations by email, roles and revocation, the permissions matrix, a POS role, and every
  property setting editable in the UI.
- **8b:** Channex channels ([phase-8-settings-channels.md](phase-8-settings-channels.md), "Channels"). It needs a
  Channex sandbox account first.

Payment providers and routing settings come with 7c. IBE branding comes with Phase 9. Caching (Redis) is deferred
until the system runs with real users (the owner, 2026-10-25).

"Builder's call" marks a decision that was not discussed with the owner.

## Done when

- **Owners invite staff by email.** The invitation names a role, either tenant-wide or for chosen properties, and goes
  out through AWS SES in production. Opening the link lets the person set a password, or sign in if they already have
  an account, and join the tenant with those roles.
- **Owners change and revoke access.** They can change a staff member's roles and revoke a membership. Revocation ends
  that user's sessions for the tenant at once; their next request gets `401`.
- **A permissions matrix** shows what each role can do.
- **A POS role** holds `PosPost` alone, so a POS no longer signs in as a manager.
- **Every property setting this phase owns is editable in the UI:**
  - name;
  - check-in, check-out and audit times;
  - taxes and invoice details, including a new SLTDA licence number;
  - accepted currencies;
  - outlets, exchange rates and laundry prices (these already exist);
  - block reason codes;
  - housekeeping sections (these already exist on Rooms);
  - cancellation policies (these already exist on Rate plans).

  They are reachable from one Settings area.

## Invitations

- **`invitation` table**, per tenant:
  - email (`citext`), display name;
  - the grants to give, a list of `{role, property_id | null}`;
  - who invited and when;
  - `token_hash` (SHA-256 of a 32-byte random token; the token itself is never stored);
  - `expires_at` (7 days);
  - status `pending`, `accepted`, `revoked`, `expired`, with `accepted_at` and `accepted_user_id`.

  There is at most one `pending` invitation per tenant and email.
- **Creating** (`StaffManage`) is idempotent on `Idempotency-Key`.
  - It refuses an email that is already a member of the tenant (`409`).
  - It refuses the `owner` role (`422`). Owners are not invited in 8a; the tenant's creator is its owner.
  - It writes the invitation and an outbox row on the topic `mail-send` in the same transaction.
- **Resending** (`StaffManage`) issues a new token, moves `expires_at` to 7 days from now, and sends again. The old
  link stops working.
- **Revoking** a pending invitation (`StaffManage`) makes its link refuse with "This invitation is no longer valid".
- **Accepting** (`POST /api/v1/invitations/accept` with the token, public, CSRF-exempt like sign-up, and throttled like
  sign-in):
  1. Hash the token and find a `pending` invitation that has not expired. Otherwise answer `410` "This invitation is no
     longer valid". The same answer is given for an unknown token, so tokens cannot be probed.
  2. **If no account has that email:** the body must carry a password (same rules as sign-up). Create the `app_user`.
  3. **If an account has that email:** the caller must be signed in as that user. Otherwise answer `409` "Sign in as
     <email> to accept". Their password is not changed.
  4. In one transaction: create the membership and the grants, mark the invitation `accepted`, and audit
     `invitation.accepted`. A new user gets a new session for that tenant. A signed-in existing user's current session
     switches to the new tenant, as `switch_tenant` does. **Builder's call:** sign-in must work for a user with no
     memberships (one removed from their only tenant), landing on an empty tenant list, so that they can accept.

  A token works once: a second accept gets `410`.
- **Reading:** `GET /invite/<token>` in the SPA shows the tenant's name, the inviter and the roles, through a public
  `POST /api/v1/invitations/preview` that answers `410` like accept. It then offers "Set a password" or "Sign in to
  accept".

## Email

- **A `mail` crate** with a `Mailer` trait: `send(Message { to, subject, text, html })`. It has two implementations:
  - `SesMailer`, using the SES v2 `SendEmail` API over HTTPS with SigV4. Configured by `MAIL_FROM`, `AWS_REGION`,
    `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, or the instance's role where the platform supplies one.
  - `DevMailer`, which writes each message as a JSON file to `MAIL_OUTBOX_DIR` (default `target/mail-outbox`), and logs
    the recipient and subject, never the token.

  `APP_ENV=production` requires `SesMailer`. **Builder's call:** the client is the smallest that cargo-deny accepts,
  either the official `aws-sdk-sesv2` or `aws-sigv4` with `reqwest`. The builder checks both and records the choice.
- **Sending goes through the outbox.** core-api's relay pushes `mail-send` to jobs-svc's new `POST /mail` endpoint
  (svc-auth), which renders and sends. A failure is retried by the bus as other topics are. **Builder's call:** sending
  in jobs-svc keeps SES credentials out of core-api's request path.
- **The invitation email** has:
  - subject "<inviter> invited you to <tenant> on GoodFolk";
  - the properties and roles in plain words;
  - the link `<APP_BASE_URL>/invite/<token>`;
  - the expiry date;
  - text and simple HTML versions;
  - the token only in the link.
- **The e2e suite** reads the dev outbox directory to follow the link.

## Staff and roles

- **The Staff page** (Settings, `StaffManage`) lists the tenant's members: name, email, grants (role and property, or
  "All properties"), and when they joined. Pending invitations are listed beside them.
- **Changing grants** replaces a member's grants and audits the previous and new grants.
  - It is `If-Match` on a new `membership.version` column, which also bumps on every other membership change.
  - It takes effect on the member's next request: grants are re-read per request, so open sessions are kept.
    **Builder's call.**
  - Every grant, invited or edited, names a role and either no property (tenant-wide) or a property of the tenant
    (`422` otherwise). A member may hold several grants, such as the same role on two properties. Any role may be
    tenant-wide or per property, except `owner`, which is always tenant-wide.
  - The tenant's last owner can't lose the `owner` grant (`409`).
  - Nobody changes their own grants.
- **Revoking** a membership, i.e. removing someone from the tenant:
  - it deletes the membership, its grants (cascade), and the user's sessions whose `active_tenant_id` is this tenant;
  - the last owner can't be removed;
  - an owner can't remove themself;
  - it is audited `membership.revoked`;
  - the user's next request with an old session cookie is `401`.
- **Roles are fixed** (no custom roles in v1): owner, manager, front_desk, housekeeping, hk_supervisor, accountant,
  and the new **`pos`** role, which allows `PosPost` only.
- **A POS is an API client, not a screen user.** **Builder's call:** `PosPost` also allows the two reads a till needs,
  the property's active outlets and GraphQL `posInHouse(propertyId)` (in-house stays with `reservation_room` id, room
  number and guest name, and nothing else). No SPA pages are shown to a `pos`-only user. "Nothing else" is tested
  against every other route group.
- **The new permission `StaffManage`** goes to the owner only. **Builder's call;** see "Questions".
- **The permissions matrix** is a read-only page of roles × permissions, in the spec's own wording ("Allows: …"),
  generated from `rbac.rs` so it cannot drift. It is served by GraphQL `permissionMatrix` and is readable by every
  member. **Builder's call:** it reveals nothing tenant-specific.
- **Settings pages keep their existing permissions.** The index shows exactly the pages the user can open.
- **The interim `add-staff` command** stays as a development and seeding tool, documented as such. **Builder's call:**
  the e2e helpers keep using it for speed, and only the invitation tests go through email.

## Settings

- **A Settings index page** lists every settings page the user may see, grouped by:
  - Property: General, Taxes and invoices, Currencies and exchange rates;
  - Operations: Sections, Block reasons, Outlets, Laundry prices;
  - Rates: Cancellation policies, which links to Rate plans;
  - Staff: Staff, Permissions.
- **General** (`PropertiesManage`): name, check-in and check-out times, and audit time, with `If-Match` on the
  property. The time zone and base currency are shown read-only. **Builder's call:** changing them would move the
  business date and every stored base amount, so it needs its own design.
- **Taxes and invoices** (the existing Taxes page) gains `sltda_licence` (text, at most 50 characters). It is printed on
  invoices under the TIN when set: the invoice template and `InvoiceData` gain the field.
- **Block reasons** (`RoomsManage`): list, add, rename and deactivate, using the existing REST routes.
- **Sections** keep their place on Rooms, and the index links there.

## API

- **REST:**
  - `POST /api/v1/invitations` (idempotent);
  - `POST /api/v1/invitations/{id}/resend`;
  - `POST /api/v1/invitations/{id}/revoke`;
  - `POST /api/v1/invitations/preview` and `POST /api/v1/invitations/accept` (public);
  - `PUT /api/v1/members/{user}/grants` (`If-Match`);
  - `DELETE /api/v1/members/{user}`;
  - the property `PATCH` takes `name`;
  - the tax settings `PATCH` takes `sltda_licence`.
- **GraphQL:** `members`, `invitations(status)`, `permissionMatrix`, and the existing settings reads.
- **Events:** `staff:<tenant>` for members and invitations.
- **Audit:** `invitation.created`, `invitation.resent`, `invitation.revoked`, `invitation.accepted`,
  `membership.grants_changed`, `membership.revoked`, `property.updated` (name included).

## Tests that must exist

- **Tokens:**
  - an invitation token works once and not after expiry;
  - a resent invitation's old token is refused;
  - an unknown token gets the same `410`.
- **Accepting:**
  - a new user accepts, sets a password and is signed in with exactly the grants invited;
  - an existing user must be signed in as that email.
- **Revoking:**
  - a revoked user's next request is `401`;
  - the last owner can't be removed or demoted.
- **The POS role** can post a POS bill and nothing else.
- **Mail:** the outbox carries the mail. The dev mailer writes the message, with a link that works and no token in the
  log. The SES mailer's request signing matches AWS's published SigV4 test vector.
- **Isolation:** the isolation suite covers `invitation`.
- **e2e:**
  - invite a front-desk clerk, open the link from the dev outbox, set a password and land in the property;
  - revoke them, and their open tab is signed out.

## Questions (for the owner)

1. Who may invite and manage staff: the owner only, or managers too (for example, managers inviting front desk and
   housekeeping but not managers)?
2. ARCHITECTURE §7.7 lists a deposit policy among property policies, and none exists yet. Is a deposit policy (such as
   "take N% or N nights at booking") wanted in 8a, or later?
