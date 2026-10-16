# Phase 7c: Online Payment Gateways and Bank Guides (Spec)

**Depends on:** Phase 7a ([phase-7-folio-audit.md](phase-7-folio-audit.md)). **Needs from the owner first:** sandbox
merchant accounts for CyberSource and MPGS, plus payments.lk if it is wanted. **Implementation plan:** written at phase
start. Design: [ARCHITECTURE.md §10.1](../ARCHITECTURE.md).

## Done when

- Payments are taken online through CyberSource or the Mastercard Gateway (MPGS), whichever the hotel's bank provides, or through payments.lk for LKR. Refunds and voids work.
- Setup guides exist for CyberSource, MPGS and each major bank.

## Components

- **The `PaymentProvider` trait** (7a's desk payments become the `manual` provider). Three online providers:
  - **CyberSource:** Unified Checkout, TMS tokens, JWT auth, encrypted webhooks.
  - **MPGS:** Hosted Checkout, tokens, Basic auth, a gateway host per property, `X-Notification-Secret` plus an order re-fetch.
  - **payments.lk.**
- **Webhooks.** One webhook ingestion endpoint per provider under `/webhooks/`.
- **Routing.** Routing rules per property: at the desk or online, currency, residency. Per-property merchant credentials are kept in Secret Manager. Card data never touches our servers.

## Bank setup guides (`docs/guides/payments/`)

`cybersource.md` and `mpgs.md`, plus one page per bank: ComBank, HNB, Sampath, BOC, People's, Seylan and NTB. NDB, DFCC and Pan Asia get a page once their platform is confirmed. The pages are written from each bank's current merchant documentation and checked with the bank's merchant team before hotels receive them.

## Tests that must exist

- **Provider contracts:** tests against recorded sandbox responses for CyberSource, MPGS and payments.lk.
- **Webhooks:** signature and secret verification; a replayed webhook is idempotent.
