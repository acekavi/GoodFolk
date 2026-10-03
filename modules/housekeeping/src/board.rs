//! The housekeeping board and a housekeeper's own rooms: each room's condition, occupancy, assignee, open
//! issues and active block, read in one statement.

use crate::{Condition, Housekeeper, Occupancy};
use db::{Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct BoardRoom {
    pub room_id: Uuid,
    pub number: String,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
    pub room_type_id: Uuid,
    pub condition: Condition,
    /// 0 for a room with no condition row, which no write can match.
    pub condition_version: i32,
    pub occupancy: Occupancy,
    /// Who cleans the room on the business date.
    pub assignee: Option<Housekeeper>,
    pub open_issues: i32,
    /// The kind of the block covering the business date, if any.
    pub block_kind: Option<rooms::BlockKind>,
}

#[derive(sqlx::FromRow)]
struct Row {
    room_id: Uuid,
    number: String,
    floor: Option<String>,
    section_id: Option<Uuid>,
    room_type_id: Uuid,
    #[sqlx(try_from = "String")]
    condition: Condition,
    condition_version: i32,
    #[sqlx(try_from = "String")]
    occupancy: Occupancy,
    assignee_id: Option<Uuid>,
    assignee_name: Option<String>,
    open_issues: i32,
    block_kind: Option<String>,
}

impl TryFrom<Row> for BoardRoom {
    type Error = sqlx::Error;

    fn try_from(row: Row) -> Result<Self, Self::Error> {
        let block_kind = row
            .block_kind
            .map(|kind| {
                rooms::BlockKind::try_from(kind)
                    .map_err(|source| sqlx::Error::ColumnDecode { index: "block_kind".into(), source: source.into() })
            })
            .transpose()?;
        let assignee =
            row.assignee_id.zip(row.assignee_name).map(|(user_id, display_name)| Housekeeper { user_id, display_name });
        Ok(BoardRoom {
            room_id: row.room_id,
            number: row.number,
            floor: row.floor,
            section_id: row.section_id,
            room_type_id: row.room_type_id,
            condition: row.condition,
            condition_version: row.condition_version,
            occupancy: row.occupancy,
            assignee,
            open_issues: row.open_issues,
            block_kind,
        })
    }
}

/// Every active room of the property with its derived state, in rail order (room type, room, then its number:
/// digits-only numbers by value, the others as text); `$2` limits it to the rooms assigned to that user today.
///
/// The stay and block conditions compare stored date and integer columns, which are leakproof, so under
/// row-level security they can be index conditions (see 0011). The 31 is the short-stay limit of
/// `reservation_room_long_stay_idx` (0011) and `room_block_long_idx` (0012): a shorter stay that reaches the
/// business date arrived at most 31 days before it, and a longer one is found through its own index.
/// A room with no `room_condition` row counts as clean at version 0. A checked-out stay's range was shrunk at check-out, so it makes the room occupied only while
/// the business date is before its end and never due out: an early departure leaves the room vacant, and it never counts as arriving, even on its
/// arrival day. A stay that arrived today and is checked in is in the house, not arriving.
const SQL: &str = "
select r.id as room_id, r.number, r.floor, r.section_id, r.room_type_id,
       coalesce(c.condition, 'clean') as condition, coalesce(c.version, 0) as condition_version,
       case when o.departs and o.arrives then 'turnover'
            when o.departs then 'due_out'
            when o.arrives then 'due_in'
            when o.in_house then 'stay_over'
            else 'vacant' end as occupancy,
       a.user_id as assignee_id, u.display_name as assignee_name,
       coalesce(i.open, 0)::int as open_issues, b.kind as block_kind
from property p
join room r on r.property_id = p.id and r.active
join room_type rt on rt.id = r.room_type_id
left join room_condition c on c.room_id = r.id
left join hk_assignment a on a.property_id = p.id and a.business_date = p.business_date and a.room_id = r.id
left join app_user u on u.id = a.user_id
left join lateral (
  select count(*) as open from room_issue x where x.property_id = p.id and x.room_id = r.id and x.status = 'open'
) i on true
left join lateral (
  select k.kind from room_block k
  where k.room_id = r.id and k.released_at is null and k.starts <= p.business_date
    and k.starts + k.days > p.business_date
    and ((k.days <= 31 and k.starts >= p.business_date - 31) or k.days > 31)
  order by k.starts limit 1
) b on true
cross join lateral (
  select coalesce(bool_or(s.arrival + s.nights = p.business_date and s.status <> 'checked_out'), false) as departs,
         coalesce(bool_or(s.arrival = p.business_date and s.status not in ('checked_in', 'checked_out')), false) as arrives,
         coalesce(bool_or((s.arrival < p.business_date or (s.arrival = p.business_date and s.status = 'checked_in'))
                          and s.arrival + s.nights > p.business_date), false) as in_house
  from reservation_room s
  where s.room_id = r.id and s.status not in ('cancelled', 'no_show')
    and s.arrival <= p.business_date
    and ((s.nights <= 31 and s.arrival >= p.business_date - 31) or s.nights > 31)
) o
where p.id = $1 and ($2::uuid is null or a.user_id = $2)
order by rt.sort_order, r.sort_order, (r.number !~ '^[0-9]+$'),
         case when r.number ~ '^[0-9]+$' then length(r.number) end, r.number, r.id";

async fn read(tx: &mut Tx, property: Uuid, user: Option<UserId>) -> Result<Vec<BoardRoom>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(SQL).bind(property).bind(user.map(|user| user.0)).fetch_all(&mut **tx).await?;
    rows.into_iter().map(BoardRoom::try_from).collect()
}

/// Every active room of the property, in rail order.
pub async fn board(tx: &mut Tx, property: Uuid) -> Result<Vec<BoardRoom>, sqlx::Error> {
    read(tx, property, None).await
}

/// The rooms assigned to `user` for the business date: turnovers, departures, stay-overs, arrivals, then empty
/// rooms, each group in rail order.
pub async fn my_rooms(tx: &mut Tx, property: Uuid, user: UserId) -> Result<Vec<BoardRoom>, sqlx::Error> {
    let mut rooms = read(tx, property, Some(user)).await?;
    rooms.sort_by_key(|room| room.occupancy.cleaning_rank());
    Ok(rooms)
}
