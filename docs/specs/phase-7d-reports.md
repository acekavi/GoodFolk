# Phase 7d: Reports and Liability Reports (Spec outline)

**Depends on:** Phase 7b ([phase-7b-night-audit.md](phase-7b-night-audit.md)), whose `daily_stats` and
`daily_stats_breakdown` snapshots these reports read. **Spec and plan:** written in full at phase start. Split from 7b by
the owner (2026-10-20).

## Done when

- **Core reports exist**, on screen and as PDF: occupancy, ADR and RevPAR over a range; revenue by segment, source, residency, room type and outlet; arrivals, departures and in-house lists.
- **Liability reports exist:**
  - TDL quarterly (SLTDA);
  - SSCL monthly and quarterly;
  - VAT per period;
  - service charge collected.
- **The close renders reports.** It gains a `render_reports` step, before the business-date advance. The PDFs are rendered by media-svc and stored privately.

## Open before the spec

- The layouts the SLTDA and IRD expect, and the open accountant questions in [ARCHITECTURE.md §14](../ARCHITECTURE.md).
