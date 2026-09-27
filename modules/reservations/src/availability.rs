//! What a property can sell for a stay: free rooms per room type from the inventory counters, and every offer
//! priced by [`rates::load_offers`].

use crate::{ReservationsError, business_date, check_window};
use db::Tx;
use rates::{Offer, OfferRequest, Residency};
use serde::Serialize;
use std::collections::HashMap;
use time::Date;
use uuid::Uuid;

/// Most nights one availability search covers.
pub const MAX_AVAILABILITY_NIGHTS: i64 = 30;

/// A stay to search: `[check_in, check_out)` for `adults` and `children` of `residency`.
#[derive(Debug, Clone)]
pub struct AvailabilityRequest {
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub residency: Residency,
}

/// An active room type: how many of its rooms are free on every night of the stay, and every offer for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RoomTypeAvailability {
    pub room_type_id: Uuid,
    pub code: String,
    pub name: String,
    /// The fewest rooms free (`physical - sold - out_of_order`) on any night of the stay; negative when
    /// overbooked.
    pub free: i32,
    /// Sorted by plan code, then meal plan; unsellable offers carry their violations.
    pub offers: Vec<Offer>,
}

#[derive(sqlx::FromRow)]
struct FreeRow {
    id: Uuid,
    code: String,
    name: String,
    /// Nights of the stay that have a counter row.
    counted: i64,
    free: Option<i32>,
}

/// Every active room type, in display order, with its free rooms and offers for the stay. The stay must be 1
/// to [`MAX_AVAILABILITY_NIGHTS`] nights inside the counter window, `[business date, business date +
/// WINDOW_DAYS)`.
pub async fn availability(
    tx: &mut Tx,
    property: Uuid,
    request: &AvailabilityRequest,
) -> Result<Vec<RoomTypeAvailability>, ReservationsError> {
    let (check_in, check_out) = (request.check_in, request.check_out);
    if check_out <= check_in {
        return Err(ReservationsError::Invalid("check-out is after check-in".into()));
    }
    let nights = (check_out - check_in).whole_days();
    if nights > MAX_AVAILABILITY_NIGHTS {
        return Err(ReservationsError::Invalid(format!(
            "an availability search covers at most {MAX_AVAILABILITY_NIGHTS} nights"
        )));
    }
    check_window(business_date(tx, property).await?, check_in, check_out)?;

    let rows: Vec<FreeRow> = sqlx::query_as(
        "select rt.id, rt.code, rt.name, count(i.date) as counted, min(i.physical - i.sold - i.out_of_order) as free
         from room_type rt
         left join inventory_day i on i.room_type_id = rt.id and i.date >= $2 and i.date < $3
         where rt.property_id = $1 and rt.active
         group by rt.id
         order by rt.sort_order, rt.code",
    )
    .bind(property)
    .bind(check_in)
    .bind(check_out)
    .fetch_all(&mut **tx)
    .await?;

    let offer_request = OfferRequest {
        check_in,
        check_out,
        adults: request.adults,
        children: request.children,
        residency: request.residency,
        room_type_ids: Some(rows.iter().map(|row| row.id).collect()),
    };
    let mut offers: HashMap<Uuid, Vec<Offer>> = HashMap::new();
    for offer in rates::load_offers(tx, property, &offer_request).await? {
        offers.entry(offer.room_type_id).or_default().push(offer);
    }

    Ok(rows
        .into_iter()
        .map(|row| {
            // A night without a counter row (past the window's last extension) has no room to sell.
            let free = row.free.unwrap_or(0);
            let free = if row.counted < nights { free.min(0) } else { free };
            RoomTypeAvailability {
                room_type_id: row.id,
                offers: offers.remove(&row.id).unwrap_or_default(),
                code: row.code,
                name: row.name,
                free,
            }
        })
        .collect())
}
