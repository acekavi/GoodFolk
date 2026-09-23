# Phase 2: Rate Plans and Meal Plans (Spec)

**Depends on:** Phase 1. **Implementation plan:** written at phase start in `docs/superpowers/plans/`.

## Done when

- A revenue manager creates standard (parent), derived and custom rate plans, tagged `FIT-F` / `FIT-L` / `OTA` / `TA` / `IBE`, each in its own currency. Resident (`FIT-L`) prices are set by hand.
- They edit prices per room type, date and occupancy in a grid, and apply bulk changes ("+10% for July, weekends only"). Derived plans update in the same save.
- Meal plans RO/BB/HB/FB are sold on any plan that allows them, priced as per-person-per-night supplements.
- Restrictions (closed, min/max stay, closed to arrival or departure) are set per plan, room type and date, and can be inherited by derived plans.

## Scope

In: `rate_plan`, `rate_plan_room_type`, `rate_day`, `meal_supplement`, `cancellation_policy`; price quote function used by Phases 3 and 9.

Out: channel mapping (Phase 8); taxes on prices (Phase 7, where rates are treated as net of tax unless the plan is flagged tax-inclusive).

## Data

See [data-model.md § Phase 2](../design/data-model.md#phase-2-rates-and-meal-plans).

## Rules

- **Derived price** = parent price adjusted by `derive_mode` (`percent`: basis points, `amount`: minor units, both may be negative), then rounded to `rounding_step` (half-up). A derived plan uses its parent's currency; any other currency is rejected (resident prices are set by hand). Maximum depth 3; cycles are rejected.
- **Recalculation:** any write to a parent's `rate_day` rows recalculates the affected descendants' rows in the **same transaction**, set-based in SQL (one `insert … select … on conflict do update` per level). A derived plan's own restrictions win unless `inherit_restrictions`.
- **Occupancy pricing:** `rate_day.occupancy` from 1 to the room type's `max_occupancy`. Missing occupancies fall back to the nearest lower one plus the extra-adult amount (a plan setting).
- **Meal supplement** per night = `adults × adult_amount + children × child_amount` for the chosen meal plan and the plan's currency. `RO` = 0. A plan lists `allowed_meal_plans`.
- **Quote** `quote(property, room_type, rate_plan, meal_plan, stay, adults, children) -> Quote { nights: [{date, room, meal}], total, currency, restrictions_ok, violations }` is a pure function over loaded rows, unit-tested, and reused by reservations and the IBE.
- **Residency:** a plan with `residency = resident` can only be quoted for resident guests, and the same for non-resident.

## API

REST: `POST/PATCH /api/v1/properties/{p}/rate-plans[/{id}]` (`rates.manage`); `PUT …/rate-plans/{id}/prices` (a batch of `{room_type_id, date, occupancy, amount}`); `POST …/rate-plans/{id}/bulk-change` (`{from, to, weekdays, room_type_ids, change: {mode, value}}`); `PUT …/rate-plans/{id}/restrictions`; `…/meal-supplements`; `…/cancellation-policies`.

GraphQL: `ratePlans(propertyId)` (tree: parent → derived), `rateGrid(propertyId, ratePlanId, from, to) { roomTypeId date occupancy amount closed minStay … }`, `quote(…)`.

Events: `rate-plans:<p>`, `rates:<p>:<ratePlanId>:<yyyy-mm>`.

## UI

- **Rate plans:** a tree view with parent and derived plans indented, badges for segment, currency and residency, and an edit panel.
- **Rate grid:** room types × dates (virtualized horizontally, same component as the inventory grid). Plan selector. Editable cells that save on blur in batches. Derived plans are read-only and show their formula. A bulk-change dialog previews the changed cells before saving.
- **Meal supplements:** a small table per currency.

## Tests that must exist

- Derivation: property-based tests for percent and amount, negative values, rounding, depth 3, cycle rejection, currency mismatch rejection.
- Bulk change updates exactly the selected dates, weekdays and room types, and cascades to descendants.
- Quote: nights, meal supplements, restrictions (min stay across the stay, CTA on arrival date only, CTD on departure date only).
- Isolation-suite cases.

## Performance gates

- Grid of 1 plan × 12 room types × 62 days × 2 occupancies (~1.5k cells): p95 < 30 ms server time.
- Bulk change of 1 year × 12 types with 2 derived levels: < 300 ms.
