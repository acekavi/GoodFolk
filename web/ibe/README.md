# web/ibe: the booking site

The guest-facing booking site: SvelteKit 2 with Svelte 5, server-rendered by `@sveltejs/adapter-node`. Each property
whose site is enabled is served at `/<CODE>` on the booking domain, or at the root of its custom host.

```sh
bun install
bun run dev        # http://localhost:5174/<CODE>, against core-api on http://localhost:8080
bun run lint && bun run check && bun run test && bun run build
node build         # the production server (PORT, HOST; ORIGIN=http://localhost:5174 for plain HTTP locally)
```

`bun run test:e2e` books, emails and cancels through the built site and the built staff app against a real API (it
starts core-api, media-svc, jobs-svc and both web servers on ports apart from the staff app's own suite). It needs the
environment of `web/pms/README.md` "End-to-end tests" and the internet: the always-pass Turnstile test keys still ask
Cloudflare.

`bun run api:schemas && bun run codegen` regenerates `src/lib/api/openapi.d.ts` from core-api after a public API
change.

## Where the API is called from

- **The first render** runs on this server: the `load` functions call the API at `IBE_API_ORIGIN` (`handleFetch` in
  `src/hooks.server.ts` sends them there). With `IBE_SSR_SECRET` set, each call names the guest (from
  `getClientAddress()`) in `X-IBE-Client-IP`, proved by `X-IBE-SSR-Secret`, so core-api counts it against the guest.
- **Everything after** runs in the browser, against `PUBLIC_API_ORIGIN`: a changed search (the results page's load is
  universal), the booking and the cancel. core-api's per-IP limits then see the guest, and its CORS admits the
  booking domain and each enabled custom host.

The variables (`PUBLIC_API_ORIGIN`, `IBE_API_ORIGIN`, `IBE_SSR_SECRET`, `PUBLIC_IBE_ORIGIN`, `PUBLIC_TURNSTILE_SITEKEY`,
`ADDRESS_HEADER`, `XFF_DEPTH`) are described
in the root README under Configuration, Booking site.

## Pages

| Path                                                       | Page                                                                                                               |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `/<CODE>`                                                  | The landing page: hero, description, the search form (the residency question first) and the rooms.                 |
| `/<CODE>/rooms?check_in&check_out&residency&rooms=2-0,2-1` | The results: a room type, plan and meal plan per room asked. Nothing is searched before the residency is answered. |
| `/<CODE>/book?…&pick=<type>~<plan>~<meal>`                 | Details and review, priced again by a fresh search; the Turnstile widget and the booking.                          |
| `/<CODE>/booked/<confirmation>`                            | The confirmation, shown once: the manage link comes from the booking's answer, never from storage.                 |
| `/<CODE>/manage?token=…`                                   | Viewing and cancelling: `no-store`, `no-referrer`, and the token leaves the address once loaded.                   |
| `/<CODE>/sitemap.xml`                                      | The landing page, at the host it was asked on.                                                                     |

A host other than the booking domain is looked up with `GET /api/public/v1/ibe/by-host/{host}` (kept 60 s) and served
by the `reroute` hook in `src/hooks.ts`: `/rooms` there is `/<CODE>/rooms`. `/` on the booking domain, and any host
with no enabled site, is a plain 404.
