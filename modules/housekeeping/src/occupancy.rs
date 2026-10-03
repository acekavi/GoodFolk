//! What a room's stays make of it on the business date. Derived at read time, never stored.

db::text_enum!(
    /// A room's occupancy on the business date: whether a guest leaves, arrives or stays in it.
    Occupancy {
        Vacant = "vacant",
        StayOver = "stay_over",
        DueOut = "due_out",
        DueIn = "due_in",
        Turnover = "turnover"
    }
);

impl Occupancy {
    /// The order a housekeeper cleans in: turnovers first, then departures, stay-overs, arrivals and empty rooms.
    pub(crate) fn cleaning_rank(self) -> u8 {
        match self {
            Occupancy::Turnover => 0,
            Occupancy::DueOut => 1,
            Occupancy::StayOver => 2,
            Occupancy::DueIn => 3,
            Occupancy::Vacant => 4,
        }
    }
}
