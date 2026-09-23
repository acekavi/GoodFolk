# Phase 9: Internet Booking Engine (Spec, to be designed)

**Depends on:** Phases 2, 3, 7, 8. **Status:** outline only. You are still planning the IBE, so this phase starts with a design session, and this spec is rewritten from its outcome before the implementation plan is written.

## Fixed by the architecture

- A separate SvelteKit app (`web/ibe`) **with server-side rendering** for SEO and fast first paint, deployed at the edge or on Cloud Run, on a custom domain or subdomain per property (`ibe_site`).
- Public, anonymous, rate-limited read endpoints: availability and quotes from `inventory_day` + `rate_day` (the Phase 2 quote function), cached at the edge for about 30 s per `(property, dates, occupancy, residency)`.
- Guests choose **residency first**. Only matching plans are shown (FIT-F in foreign currency, FIT-L in LKR).
- Booking: hold (a tentative reservation with `hold_expires_at`, releasing inventory on expiry) → pay (CyberSource / MPGS hosted checkout; payments.lk for LKR if enabled) → confirm on webhook → confirmation email.
- No card data on our servers; PCI SAQ A.

## Questions for the design session

1. Single property per site, or chain-level search across properties?
2. Payment options: full prepayment, deposit, pay at hotel with card guarantee?
3. Upsells: meal-plan upgrades, airport transfers, add-ons?
4. Promo codes and member rates?
5. Languages and currencies shown to guests (converted display vs charged currency)?
6. Branding depth: themes only, or custom layouts?
7. Google Hotel Ads / metasearch free booking links (via Channex)?
8. Guest self-service: view, modify or cancel a booking?

## Performance targets (to confirm)

- Search results < 300 ms at the edge (p95). Largest Contentful Paint < 1.5 s on mid-range mobile. A booking completes in ≤ 3 steps.
