# Phase 9: Internet Booking Engine (Spec)

**Depends on:**
- Phase 2: rates, the quote function and meal plans.
- Phase 3: reservations, cancellation terms and guests.
- Phase 6a/6b: room type galleries, photos and tour videos.
- Phase 7: the tax rules, used for price previews.
- Phase 8a: settings, the advisory deposit rule, and mail.

**Implementation plan:** written at phase start in `docs/superpowers/plans/`.

**How this spec was made:**
- On 2026-10-28 the owner asked for it to be written from the roadmap outline and ARCHITECTURE §10, without a design session.
- On the same day the owner put online card payments (Phase 7c) and Channex (Phase 8b) on hold.
- So the eight design questions of the outline are answered here as **Builder's calls**. Each one says what was chosen and what it costs to change later.

## Intent

A guest who finds the hotel online books a room directly on the hotel's own booking site in at most three steps:
search, choose, details and book. The booking lands in the PMS as a normal reservation that front desk sees
immediately. It costs the hotel no commission, and it works before online payment exists.

## Done when

- **The booking site.** A public, server-rendered booking site (`web/ibe`) serves each property that turns it on:
  - search by dates, rooms and occupancy, after the guest chooses residency;
  - results per room type with its photos, tour video and prices;
  - guest details;
  - a confirmed booking with a confirmation page and email.
- **Managing the booking.** The guest can view or cancel the booking from a link in the email.
- **Settings.** Staff turn the site on and brand it in **Settings → Booking site**.
- **Back office.** IBE bookings show in the PMS with source `ibe`.
- The performance gates pass.

## Builder's calls on the outline's questions

1. **One property per site.** There is no chain-level search.
   - Each property's site lives at `/<PROPERTY CODE>` on the booking domain (`IBE_PUBLIC_ORIGIN`), for example
     `https://book.example.lk/GAL`.
   - A custom host per property (`book.galleforthotel.lk`) can be set and maps to the same pages.
   - Chain search can be added later, since the property is in every URL.
2. **Payment: pay at the hotel, for now.**
   - The booking is confirmed at once, with `guarantee = 'none'`. No card or online payment is taken.
   - If the rate plan has an advisory deposit rule (Phase 8a), the guest sees the amount and the due date, with "The hotel will contact you about the deposit". It is never enforced.
   - **Later, with Phase 7c:** a "pay now" stage slots in between details and confirmation: hold, pay, confirm on the webhook. The `reservation.hold_expires_at` column already exists for it. This spec does not build the hold stage, and the booking flow is shaped so it can be added without changing the guest's three steps.
3. **Upsells.** The only choice beyond the room is the **meal plan**, from the rate plan's allowed meal plans, priced by the existing quote. There are no add-ons or transfers.
4. **No promo codes and no member rates.**
5. **Language and currency.**
   - The site is in English only.
   - Prices are shown and booked in the **rate plan's own currency**: a FIT-F plan in USD or EUR, a FIT-L plan in LKR. There is no converted display.
6. **Branding is a theme, not a layout:** logo, one accent colour, a hero image and a short description.
7. **No metasearch.** Google Hotel Ads and free booking links would come through Channex, which is on hold.
8. **Guest self-service.**
   - The guest can view and cancel, but not modify. To modify, they contact the hotel; the manage page shows the hotel's phone and email.
   - Cancellation follows the booking's own cancellation terms, snapshotted at booking (Phase 3). A penalty applies exactly as it would at the front desk.

## The guest's flow

### Step 1: Search

- **Residency first.** The guest answers "Are you a resident of Sri Lanka?" before seeing any price, as ARCHITECTURE §7 requires.
- **Inputs:**
  - check-in and check-out: 1 to 30 nights, starting from the property's business date and up to 18 months ahead;
  - rooms: 1 to 4;
  - adults and children per room, within each room type's maximum occupancy.
- **Results:** each room type that has availability for every room asked for, showing:
  - its name, description, beds, amenities and maximum occupancy;
  - its gallery photos and its tour video, played on tap with sound, as in 6b.
- **Plans offered:** under each room type, every rate plan that:
  - is **segment `IBE`**;
  - has a residency matching the guest's answer, or none;
  - is open (no closed-to-arrival, min/max stay or closed violation) for the dates.

  For each plan the result shows:
  - the total for the stay;
  - a per-night breakdown;
  - the price with every tax and the service charge on the bill, as the folio engine would compute them;
  - the cancellation policy in plain words;
  - the meal plan choice, with each allowed meal plan's price.
- **Fully booked:** "No rooms available for these dates" plus the nearest available check-in date within 14 days, if one exists (**Builder's call**).

### Step 2: Choose

The guest picks a room type, a plan and a meal plan for each room asked for. The rooms can be different types.

### Step 3: Details and book

**The form:**
- **The booker:** name, email, phone, country of residence, residency (prefilled from step 1, and it must match it) and arrival time (optional).
- **Per room:** the occupant's name, optional, defaulting to the booker.
- **Special requests:** up to 500 characters, stored as the reservation's notes.

**Before booking:**
- The guest ticks agreement to the hotel's booking terms and privacy notice, both from settings.
- The form shows a summary with the total per currency, the cancellation terms and any advisory deposit.

**Booking:**
- **The command:** the booking re-quotes on the server (never trusting the browser's prices) and creates the reservation through the same reservation command front desk uses:
  - `source = 'ibe'`;
  - a new guest profile for the booker, **not** matched to existing guests by email (**Builder's call**: matching would let anyone attach a booking to someone else's profile; front desk can still see both);
  - `created_by` null.
- **Price changes:** if the price changed since the search, the guest sees the new total and must confirm again.
- **No longer available:** if the room went in the meantime, the guest is sent back to the results with "That room was just booked; here is what is still available".
- **Confirmation:** a confirmation page with the confirmation number, plus an email to the booker containing the summary, the hotel's contact details and the manage link.

### Managing the booking

- **The link:**
  - The manage link carries a 32-byte random token, of which only its SHA-256 is stored (as invitations do in 8a).
  - It is valid until 30 days after check-out.
  - A wrong or expired token gets a uniform `410 Gone`.
- **The manage page:**
  - It shows the booking.
  - It offers **Cancel booking** while every room is tentative or confirmed and check-in is in the future. The penalty, if any, is shown first.
  - Cancelling runs the existing cancellation with reason "Cancelled by the guest online" and sends a cancellation email.
- **After check-in:** once a room is checked in, the page shows the stay as read-only.

## The hotel's side

### Settings → Booking site

This page needs a new permission, `IbeManage`, held by the owner and the manager.

**The settings:**
- **Enabled** (off by default). While it is off, the site answers `404` for the property.
- **Custom host**, optional: a lowercase domain name, unique across all tenants. The page shows the DNS step: "Point a CNAME at `<booking domain>`".
- **Logo:** an image upload through the 6a media flow, new purpose `ibe_logo`, public, at most 5 MB.
- **Hero image:** chosen from the property's room type gallery photos.
- **Accent colour:** a hex colour, checked for contrast against white text at 4.5:1 or more, and refused otherwise.
- **Short description:** up to 500 characters.
- **Contact:** phone and email, shown on the site and in the emails.
- **Booking terms** and **privacy notice:** plain text, up to 10,000 characters each.

**Room type content for the site:**
- Room types gain a **description**, up to 1,000 characters, edited on the Room types page.
- The site uses the existing beds, amenities, gallery and tour videos.

**Saving:** every change is audited and saved with `If-Match`, like the other settings.

### In the PMS

- IBE bookings appear everywhere reservations do, with source **Booking site**. The reservations list can filter by source (**Builder's call**, if the filter does not exist yet).
- A new reservation from the site sends the property's front desk a live `reservations:<property>` event, as any new reservation does. There is no extra notification (**Builder's call**).
- Revenue is counted under source `ibe` in the Phase 7d reports' breakdown by source. That needs no change.

## The public API

- **Prefix:** `/api/public/v1/ibe/{code}/…`, with no session and no cookies.
- **Resolving the property:**
  - A small security-definer function resolves `{code}` (or a custom host) to `(tenant_id, property_id)` only for an **enabled** site.
  - Everything after that runs in that tenant's scope under RLS, as staff requests do.
  - An unknown or disabled code is `404`.
- **Endpoints:**
  - `GET …/site`: branding, room types and contact. Cached for 5 min.
  - `GET …/search`: the dates, rooms, occupancy and residency query. Cached `s-maxage=30` per full query string at the edge, as ARCHITECTURE §10 says.
  - `POST …/bookings`: the Turnstile token and the details; `Idempotency-Key` required, as on staff writes.
  - `GET …/bookings/manage?token=…` and `POST …/bookings/manage/cancel`.
- **Abuse limits:**
  - **Per IP:** 60 searches and 5 bookings a minute, with `429` and `Retry-After`.
  - **Bot check:** **Cloudflare Turnstile** on the booking POST (**Builder's call**: free, no image puzzles, and Cloudflare already fronts the site), verified on the server with the secret key. Development uses Turnstile's published test keys.
  - **Hidden field:** a honeypot field.
  - **Per booking:** at most 4 rooms and 30 nights.
- **Browser access:** CORS allows only `IBE_PUBLIC_ORIGIN` and the enabled custom hosts. There are no credentials, so no CSRF token is needed.
- **Guest data:**
  - A search never returns another booking's data.
  - A manage request returns only its own booking.
  - Guest details are audited (`ibe.booking_created`) without the email or phone in the audit row.

## The site (`web/ibe`)

**Technology:**
- SvelteKit with SSR.
- Bun for builds, as in `web/pms`.
- Served by Cloud Run behind Cloudflare.
- It talks only to the public API, server-side for SSR and client-side for search updates.

**Pages:**
- `/<CODE>`: the landing and search form, with the hero, the description and the room types.
- `/<CODE>/rooms?…`: the results.
- `/<CODE>/book`: details and review.
- `/<CODE>/booked/<confirmation>`: shown once, straight after booking.
- `/<CODE>/manage?token=…`: viewing and cancelling.

**SEO:**
- A real `<title>` and description.
- Open Graph tags with the hero image.
- `/<CODE>/sitemap.xml`.
- `hotel` structured data (schema.org `Hotel`).

**Mobile and accessibility:**
- Mobile first; the PMS's accessibility standard applies (labels, keyboard use, contrast).
- No cookie banner is needed, since the site sets no cookies (**Builder's call**: no analytics in this phase).

## Data

**Migrations:**
- `ibe_site`, one row per property:
  - columns: `enabled`, `custom_host` (unique when set), `logo_asset_id`, `hero_asset_id`, `accent`, `description`, `contact_phone`, `contact_email`, `terms`, `privacy` and `version`;
  - forced RLS and tenant-checked foreign keys, like every table.
- `room_type.description`.
- `reservation_access_token`: `reservation_id`, `sha256`, `expires_at` and `created_at`, with RLS.
- The `ibe_logo` media purpose.
- The definer resolving a code or host to an enabled site.
- The `IbeManage` permission.

**Mail:** two new `MailJob` variants, IBE confirmation and IBE cancellation, sent through jobs-svc like invitations, with the token redacted in `Debug`.

## Tests that must exist

**Search:**
- residency filters the plans;
- IBE-segment plans only;
- closed, min-stay and CTA violations hide a plan;
- four rooms of mixed types;
- the price includes the bill's taxes exactly as the folio engine computes them;
- a disabled site is `404`.

**Booking:**
- a booking creates a confirmed `ibe` reservation with its rooms, guests and snapshotted cancellation terms;
- a changed price needs re-confirmation;
- a sold-out race gets the "just booked" response;
- the same `Idempotency-Key` gives one booking;
- a missing or failed Turnstile check is `400`;
- the rate limits return `429`.

**Managing:**
- the token works and gives `410` once expired;
- a guest cancellation applies the snapshotted penalty and sends the email;
- a checked-in booking cannot be cancelled online.

**Isolation:** one property's code never reaches another tenant's data, and the new tables join the tenant isolation suite.

**e2e** (Playwright on `web/ibe` against the PMS stack):
- a guest searches, books two rooms and receives the confirmation email in the dev outbox;
- front desk sees the booking on the PMS reservations page;
- the guest cancels from the manage link.

## Performance gates

- **Search:** `GET …/search` for 3 nights, 2 rooms, a property with 12 room types and 5 IBE plans answers in **under 100 ms server time p95**. Uncached at the edge it is under 300 ms p95, which the outline targets for the edge.
- **Booking:** `POST …/bookings` for 2 rooms answers in under 300 ms p95.
- **Page speed:** the landing and results pages reach **Largest Contentful Paint < 1.5 s** on Lighthouse's mobile profile (throttled 4G, mid-range phone) for a property with a 10-photo gallery.
- **Steps:** a booking takes **at most 3 steps** from the results.

## Out of scope (this phase)

- **Payments:** online payment, card guarantee and the hold stage; these come with Phase 7c.
- **Rates and extras:** promo codes, member rates, packages and add-ons.
- **Reach:** chain search, metasearch and other languages.
- **Guests:** guest accounts and guest modifications.
- **Site:** custom layouts and analytics.
