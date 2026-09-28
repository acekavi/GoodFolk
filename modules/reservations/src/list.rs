//! The reservations list: one row per reservation room, filtered, sorted and paged by keyset on (sort value,
//! room id), so a page costs the same however deep it is and rows booked meanwhile never shift one.

use crate::{ReservationsError, Source};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use db::Tx;
use domain::RoomStatus;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::postgres::PgRow;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// Most rows one page holds.
pub const MAX_PAGE_SIZE: i64 = 100;

/// What the list is sorted by; each sort breaks ties by the room's id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortField {
    /// Arrival date.
    #[default]
    Arrival,
    /// Confirmation number.
    Confirmation,
    /// The primary guest's last name, then first name.
    Guest,
    /// When the reservation was made.
    Created,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sort {
    pub field: SortField,
    pub direction: SortDirection,
}

/// Which rooms to list. `None` does not filter; an empty `statuses` or `sources` matches nothing.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// First arrival date listed.
    pub arrival_from: Option<Date>,
    /// Last arrival date listed (inclusive).
    pub arrival_to: Option<Date>,
    pub statuses: Option<Vec<RoomStatus>>,
    pub sources: Option<Vec<Source>>,
    /// A confirmation number's start (any case), or a primary guest's name, typos included.
    pub text: Option<String>,
}

/// One page: `first` rows (1 to [`MAX_PAGE_SIZE`]) after the row `after` points at, a cursor from an earlier
/// page under the same sort. `count` also counts every row the filter matches.
#[derive(Debug, Clone)]
pub struct ListRequest {
    pub filter: ListFilter,
    pub sort: Sort,
    pub first: i64,
    pub after: Option<String>,
    pub count: bool,
}

/// One room of a reservation, as the list shows it. `total` is the stay's price in minor units of `currency`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRoomRow {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub confirmation_no: String,
    /// The primary guest's first and last name.
    pub guest_name: String,
    pub arrival: Date,
    pub departure: Date,
    pub nights: i32,
    pub room_type_code: String,
    pub room_number: Option<String>,
    pub status: RoomStatus,
    pub source: Source,
    pub total: i64,
    pub currency: String,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRoomPage {
    pub rows: Vec<ReservationRoomRow>,
    /// Points at the last row, for the next page's `after`; `None` on an empty page.
    pub end_cursor: Option<String>,
    pub has_next_page: bool,
    /// Every row the filter matches, on every page; `None` unless asked for.
    pub total_count: Option<i64>,
}

/// Where a page ended: the last row's sort value and id, under the sort it was read.
#[derive(Serialize, Deserialize)]
struct Cursor {
    field: SortField,
    direction: SortDirection,
    key: Key,
    id: Uuid,
}

/// A sort value, typed: comparisons on typed values are leakproof, so under row-level security Postgres may
/// use them in an index scan (a comparison through a text cast has to wait for the tenant filter).
#[derive(Serialize, Deserialize)]
enum Key {
    Date(Date),
    Text(String),
    Time(#[serde(with = "time::serde::rfc3339")] OffsetDateTime),
}

impl Cursor {
    fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).expect("a cursor serializes"))
    }

    fn decode(text: &str) -> Option<Cursor> {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(text).ok()?).ok()
    }
}

impl SortField {
    /// The sort value's SQL expression. Arrival, the default, walks `reservation_room_arrival_idx` and stops
    /// after a page. The others sort on columns of other tables (guests are tenant-wide), so they take the
    /// first rows of the property's matches in a top-N sort.
    fn key(self) -> &'static str {
        match self {
            SortField::Arrival => "rr.arrival",
            SortField::Confirmation => "r.confirmation_no",
            SortField::Guest => "lower(g.last_name || ' ' || g.first_name)",
            SortField::Created => "r.created_at",
        }
    }

    /// The sort value of `row`'s `sort_key` column.
    fn read_key(self, row: &PgRow) -> Result<Key, sqlx::Error> {
        Ok(match self {
            SortField::Arrival => Key::Date(row.try_get("sort_key")?),
            SortField::Confirmation | SortField::Guest => Key::Text(row.try_get("sort_key")?),
            SortField::Created => Key::Time(row.try_get("sort_key")?),
        })
    }

    fn fits(self, key: &Key) -> bool {
        matches!(
            (self, key),
            (SortField::Arrival, Key::Date(_))
                | (SortField::Confirmation | SortField::Guest, Key::Text(_))
                | (SortField::Created, Key::Time(_))
        )
    }
}

/// The rooms of the property's reservations that `request.filter` matches, one page of them in
/// `request.sort` order. A cursor that does not decode, or was read under another sort or direction, is
/// `Invalid`.
pub async fn list_reservation_rooms(
    tx: &mut Tx,
    property: Uuid,
    request: &ListRequest,
) -> Result<ReservationRoomPage, ReservationsError> {
    if !(1..=MAX_PAGE_SIZE).contains(&request.first) {
        return Err(ReservationsError::Invalid(format!("first is 1 to {MAX_PAGE_SIZE}")));
    }
    let sort = request.sort;
    let after = match &request.after {
        None => None,
        Some(text) => {
            let cursor = Cursor::decode(text)
                .filter(|cursor| cursor.field.fits(&cursor.key))
                .ok_or_else(|| ReservationsError::Invalid("the cursor is not valid".into()))?;
            if cursor.field != sort.field || cursor.direction != sort.direction {
                return Err(ReservationsError::Invalid(
                    "the cursor belongs to another sort; start from the first page".into(),
                ));
            }
            Some(cursor)
        }
    };

    let filter = &request.filter;
    let text = filter.text.as_deref().map(str::trim).filter(|text| !text.is_empty());
    let statuses: Option<Vec<&str>> =
        filter.statuses.as_ref().map(|statuses| statuses.iter().map(|status| status.as_str()).collect());
    let sources: Option<Vec<&str>> =
        filter.sources.as_ref().map(|sources| sources.iter().map(|source| source.as_str()).collect());
    // $1 property, $2 arrival from, $3 arrival to, $4 statuses, $5 sources, $6 confirmation prefix, $7 name.
    // The text is a confirmation number's start (starts_with, which is leakproof, so the text_pattern_ops
    // index serves it under row-level security) or a guest's name; each is looked up once, not per row.
    // The bounds are folded into the comparison (rather than `$n is null or …`) so the planner sees one plain
    // range condition on `rr.arrival` and can still use `reservation_room_arrival_idx` when a bound is left out.
    let matches = "rr.property_id = $1
         and rr.arrival >= coalesce($2, '-infinity'::date)
         and rr.arrival <= coalesce($3, 'infinity'::date)
         and ($4::text[] is null or rr.status = any($4))
         and ($5::text[] is null or r.source = any($5))
         and ($6::text is null
              or rr.reservation_id = any(array(
                   select id from reservation where property_id = $1 and starts_with(confirmation_no, $6)))
              or rr.primary_guest_id = any(array(
                   select id from guest where lower($7) <% lower(first_name || ' ' || last_name))))";
    let rooms = "reservation_room rr join reservation r on r.id = rr.reservation_id";

    let key = sort.field.key();
    let (order, compare) = match sort.direction {
        SortDirection::Asc => ("asc", ">"),
        SortDirection::Desc => ("desc", "<"),
    };
    // $8 page size (plus one, to tell whether another page follows), $9 and $10 the cursor's key and id. The
    // plain bound on the key lets an index on it start at the cursor; the row comparison breaks ties.
    let keyset = if after.is_some() {
        format!("and {key} {compare}= $9 and ({key}, rr.id) {compare} ($9, $10)")
    } else {
        String::new()
    };
    let sql = format!(
        "select rr.id, rr.reservation_id, r.confirmation_no, g.first_name, g.last_name, rr.arrival,
                upper(rr.stay) as departure, rt.code as room_type_code, room.number as room_number, rr.status,
                r.source, rr.currency, rr.version, {key} as sort_key,
                (select coalesce(sum(n.room_amount + n.meal_amount), 0)::bigint
                 from reservation_night n where n.reservation_room_id = rr.id) as total
         from {rooms}
         join guest g on g.id = rr.primary_guest_id
         join room_type rt on rt.id = rr.room_type_id
         left join room on room.id = rr.room_id
         where {matches} {keyset}
         order by {key} {order}, rr.id {order}
         limit $8"
    );
    let confirmation = text.map(str::to_uppercase);
    let query = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(property)
        .bind(filter.arrival_from)
        .bind(filter.arrival_to)
        .bind(&statuses)
        .bind(&sources)
        .bind(&confirmation)
        .bind(text)
        .bind(request.first + 1);
    let query = match after {
        None => query,
        Some(Cursor { key: Key::Date(key), id, .. }) => query.bind(key).bind(id),
        Some(Cursor { key: Key::Text(key), id, .. }) => query.bind(key).bind(id),
        Some(Cursor { key: Key::Time(key), id, .. }) => query.bind(key).bind(id),
    };
    let rows: Vec<PgRow> = query.fetch_all(&mut **tx).await?;

    let has_next_page = rows.len() as i64 > request.first;
    let mut page = Vec::with_capacity(rows.len());
    let mut end_cursor = None;
    for row in rows.iter().take(request.first as usize) {
        let parsed = parse_row(row)?;
        let key = sort.field.read_key(row)?;
        end_cursor = Some(Cursor { field: sort.field, direction: sort.direction, key, id: parsed.id }.encode());
        page.push(parsed);
    }

    let total_count = if request.count {
        let sql = format!("select count(*) from {rooms} where {matches}");
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(property)
            .bind(filter.arrival_from)
            .bind(filter.arrival_to)
            .bind(&statuses)
            .bind(&sources)
            .bind(&confirmation)
            .bind(text)
            .fetch_one(&mut **tx)
            .await?;
        Some(count)
    } else {
        None
    };

    Ok(ReservationRoomPage { rows: page, end_cursor, has_next_page, total_count })
}

fn parse_row(row: &PgRow) -> Result<ReservationRoomRow, sqlx::Error> {
    let status: String = row.try_get("status")?;
    let source: String = row.try_get("source")?;
    let first_name: String = row.try_get("first_name")?;
    let last_name: String = row.try_get("last_name")?;
    let arrival: Date = row.try_get("arrival")?;
    let departure: Date = row.try_get("departure")?;
    Ok(ReservationRoomRow {
        id: row.try_get("id")?,
        reservation_id: row.try_get("reservation_id")?,
        confirmation_no: row.try_get("confirmation_no")?,
        guest_name: if first_name.is_empty() { last_name } else { format!("{first_name} {last_name}") },
        arrival,
        departure,
        nights: (departure - arrival).whole_days() as i32,
        room_type_code: row.try_get("room_type_code")?,
        room_number: row.try_get("room_number")?,
        status: RoomStatus::parse(&status).ok_or_else(|| crate::decode_error("status", &status))?,
        source: Source::parse(&source).ok_or_else(|| crate::decode_error("source", &source))?,
        total: row.try_get("total")?,
        currency: row.try_get("currency")?,
        version: row.try_get("version")?,
    })
}
