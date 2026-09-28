//! Constraints the Phase 3b migration puts on accounts, additional occupants, the overbooking allowance and
//! check-in/out columns, checked directly in the database.

use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    room_type: Uuid,
    room: Uuid,
    plan: Uuid,
    guest: Uuid,
    reservation: Uuid,
    reservation_room: Uuid,
}

/// A tenant with one property, room type, room, rate plan, guest, reservation and one confirmed
/// `reservation_room` for it. Runs as the superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel {
        tenant: Uuid::now_v7(),
        property: Uuid::now_v7(),
        room_type: Uuid::now_v7(),
        room: Uuid::now_v7(),
        plan: Uuid::now_v7(),
        guest: Uuid::now_v7(),
        reservation: Uuid::now_v7(),
        reservation_room: Uuid::now_v7(),
    };
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(hotel.tenant).execute(pool).await.unwrap();
    let statements = [
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($2, $1, $8, 'Hotel', 'Asia/Colombo', 'LKR', current_date)",
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         values ($3, $1, $2, 'DLX', 'Deluxe', 2, 2, 0, 2)",
        "insert into room (id, tenant_id, property_id, room_type_id, number) values ($4, $1, $2, $3, '101')",
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
         values ($5, $1, $2, 'BAR', 'Best available', 'standard', 'IBE', 'USD')",
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($6, $1, 'Ada', 'Silva', 'resident')",
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
         values ($7, $1, $2, $8 || '-000001', 'front_desk', $6)",
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay, adults,
                                       children, rate_plan_id, meal_plan, status, primary_guest_id, currency)
         values ($9, $1, $2, $7, $3, $4, daterange(current_date, current_date + 2), 2, 0, $5, 'RO', 'confirmed', $6, 'USD')",
    ];
    for statement in statements {
        sqlx::query(statement)
            .bind(hotel.tenant)
            .bind(hotel.property)
            .bind(hotel.room_type)
            .bind(hotel.room)
            .bind(hotel.plan)
            .bind(hotel.guest)
            .bind(hotel.reservation)
            .bind(code)
            .bind(hotel.reservation_room)
            .execute(pool)
            .await
            .unwrap();
    }
    hotel
}

fn constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|db_err| db_err.constraint()).unwrap_or_default().to_owned()
}

fn code(result: &Result<impl std::fmt::Debug, sqlx::Error>) -> Option<String> {
    result.as_ref().err().and_then(|e| e.as_database_error()).and_then(|e| e.code()).map(|c| c.into_owned())
}

// -- account --------------------------------------------------------------------------------------------------

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_is_a_company_or_a_travel_agent(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |kind: &'static str| {
        sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, $3, 'Acme', 'USD')")
            .bind(Uuid::now_v7())
            .bind(hotel.tenant)
            .bind(kind)
            .execute(&pool)
    };

    let invalid = account("wholesaler").await;
    let company = account("company").await;
    let travel_agent = account("travel_agent").await;

    assert_eq!(constraint(invalid), "account_kind_check");
    assert!(company.is_ok() && travel_agent.is_ok(), "{company:?} {travel_agent:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_name_is_one_to_two_hundred_characters(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |name: String| {
        sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', $3, 'USD')")
            .bind(Uuid::now_v7())
            .bind(hotel.tenant)
            .bind(name)
            .execute(&pool)
    };

    let empty = account(String::new()).await;
    let too_long = account("A".repeat(201)).await;
    let two_hundred = account("A".repeat(200)).await;

    assert_eq!(constraint(empty), "account_name_check");
    assert_eq!(constraint(too_long), "account_name_check");
    assert!(two_hundred.is_ok(), "{two_hundred:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn account_contact_must_be_a_json_object(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |contact: serde_json::Value| {
        sqlx::query(
            "insert into account (id, tenant_id, kind, name, contact, currency)
             values ($1, $2, 'company', 'Acme', $3, 'USD')",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(Json(contact))
        .execute(&pool)
    };

    let array = account(serde_json::json!(["not", "an", "object"])).await;
    let string = account(serde_json::json!("not an object either")).await;
    let object = account(serde_json::json!({"email": "a@example.com", "phone": "+94 11 234 5678"})).await;
    let default_empty = sqlx::query(
        "insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', 'USD')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant)
    .execute(&pool)
    .await;

    assert_eq!(constraint(array), "account_contact_check");
    assert_eq!(constraint(string), "account_contact_check");
    assert!(object.is_ok() && default_empty.is_ok(), "{object:?} {default_empty:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_credit_limit_is_never_negative(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |credit_limit: Option<i64>| {
        sqlx::query(
            "insert into account (id, tenant_id, kind, name, credit_limit, currency)
             values ($1, $2, 'company', 'Acme', $3, 'USD')",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(credit_limit)
        .execute(&pool)
    };

    let negative = account(Some(-1)).await;
    let zero = account(Some(0)).await;
    let unlimited = account(None).await;

    assert_eq!(constraint(negative), "account_credit_limit_check");
    assert!(zero.is_ok() && unlimited.is_ok(), "{zero:?} {unlimited:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_currency_is_a_three_letter_uppercase_code(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = |currency: &'static str| {
        sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', $3)")
            .bind(Uuid::now_v7())
            .bind(hotel.tenant)
            .bind(currency)
            .execute(&pool)
    };

    let lowercase = account("usd").await;
    let too_short = account("US").await;
    let valid = account("LKR").await;

    assert_eq!(constraint(lowercase), "account_currency_check");
    assert_eq!(constraint(too_short), "account_currency_check");
    assert!(valid.is_ok(), "{valid:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_is_active_by_default(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let id = Uuid::now_v7();
    sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', 'USD')")
        .bind(id)
        .bind(hotel.tenant)
        .execute(&pool)
        .await
        .unwrap();

    let active: bool =
        sqlx::query_scalar("select active from account where id = $1").bind(id).fetch_one(&pool).await.unwrap();

    assert!(active);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn deleting_an_account_referenced_by_a_reservation_is_refused(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let account = Uuid::now_v7();
    sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', 'USD')")
        .bind(account)
        .bind(hotel.tenant)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update reservation set account_id = $1 where id = $2")
        .bind(account)
        .bind(hotel.reservation)
        .execute(&pool)
        .await
        .unwrap();

    let delete = sqlx::query("delete from account where id = $1").bind(account).execute(&pool).await;

    assert_eq!(code(&delete).as_deref(), Some("23503"));
}

// -- reservation_guest ------------------------------------------------------------------------------------------

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservation_guest_cannot_use_another_propertys_room(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    // galle's room, but claimed under kandy's property: the composite FK requires both to agree.
    let cross_property = sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         values ($1, $2, $3, $4)",
    )
    .bind(kandy.tenant)
    .bind(kandy.property)
    .bind(galle.reservation_room)
    .bind(kandy.guest)
    .execute(&pool)
    .await;

    assert_eq!(code(&cross_property).as_deref(), Some("23503"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservation_guest_cannot_name_another_tenants_guest(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    // galle's own room, but the guest belongs to kandy: the composite FK requires (tenant_id, guest_id) to exist.
    let cross_tenant = sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         values ($1, $2, $3, $4)",
    )
    .bind(galle.tenant)
    .bind(galle.property)
    .bind(galle.reservation_room)
    .bind(kandy.guest)
    .execute(&pool)
    .await;

    assert_eq!(code(&cross_tenant).as_deref(), Some("23503"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn deleting_the_room_cascades_to_its_additional_occupants(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let occupant = Uuid::now_v7();
    sqlx::query("insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, 'Eve', 'Perera', 'resident')")
        .bind(occupant)
        .bind(hotel.tenant)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id) values ($1, $2, $3, $4)",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.reservation_room)
    .bind(occupant)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("delete from reservation_room where id = $1")
        .bind(hotel.reservation_room)
        .execute(&pool)
        .await
        .unwrap();

    let remaining: i64 = sqlx::query_scalar("select count(*) from reservation_guest where reservation_room_id = $1")
        .bind(hotel.reservation_room)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn deleting_a_guest_referenced_by_reservation_guest_is_refused(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let occupant = Uuid::now_v7();
    sqlx::query("insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, 'Eve', 'Perera', 'resident')")
        .bind(occupant)
        .bind(hotel.tenant)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id) values ($1, $2, $3, $4)",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.reservation_room)
    .bind(occupant)
    .execute(&pool)
    .await
    .unwrap();

    let delete = sqlx::query("delete from guest where id = $1").bind(occupant).execute(&pool).await;

    assert_eq!(code(&delete).as_deref(), Some("23503"));
}

// -- room_type.overbooking --------------------------------------------------------------------------------------

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn overbooking_is_zero_to_twenty(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    // Each attempt uses its own code: a rejected insert never commits, but the two accepted ones would
    // otherwise collide on `unique (property_id, code)`.
    let room_type = |type_code: &'static str, overbooking: i32| {
        sqlx::query(
            "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children,
                                    max_occupancy, overbooking)
             values ($1, $2, $3, $4, 'Standard', 1, 1, 0, 1, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(type_code)
        .bind(overbooking)
        .execute(&pool)
    };

    let negative = room_type("NEG", -1).await;
    let too_high = room_type("HIGH", 21).await;
    let low_bound = room_type("LOW", 0).await;
    let high_bound = room_type("MAX", 20).await;
    let default_zero: i32 = sqlx::query_scalar("select overbooking from room_type where id = $1")
        .bind(hotel.room_type)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(constraint(negative), "room_type_overbooking_check");
    assert_eq!(constraint(too_high), "room_type_overbooking_check");
    assert!(low_bound.is_ok() && high_bound.is_ok(), "{low_bound:?} {high_bound:?}");
    assert_eq!(default_zero, 0, "overbooking defaults to 0 for the room type the hotel() helper inserts");
}

// -- reservation_room check-in/out columns ----------------------------------------------------------------------

/// Sets `status`, `checked_in_at` (from `checked_in`), `checked_in_business_date` (from `business_date`) and
/// `checked_out_at` (from `checked_out`) on `hotel`'s room.
async fn set_checkin(
    pool: &PgPool,
    hotel: &Hotel,
    status: &'static str,
    checked_in: bool,
    business_date: bool,
    checked_out: bool,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query(
        "update reservation_room
         set status = $2,
             checked_in_at = case when $3 then now() end,
             checked_in_business_date = case when $4 then current_date end,
             checked_out_at = case when $5 then now() end
         where id = $1",
    )
    .bind(hotel.reservation_room)
    .bind(status)
    .bind(checked_in)
    .bind(business_date)
    .bind(checked_out)
    .execute(pool)
    .await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checked_in_at_is_set_exactly_when_the_room_is_checked_in_or_out(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let confirmed_with_it = set_checkin(&pool, &hotel, "confirmed", true, true, false).await;
    let checked_in_without_it = set_checkin(&pool, &hotel, "checked_in", false, false, false).await;
    let checked_out_without_it = set_checkin(&pool, &hotel, "checked_out", false, false, true).await;
    let no_show_with_it = set_checkin(&pool, &hotel, "no_show", true, true, false).await;
    let checked_in_with_it = set_checkin(&pool, &hotel, "checked_in", true, true, false).await;
    let confirmed_without_it = set_checkin(&pool, &hotel, "confirmed", false, false, false).await;

    assert_eq!(constraint(confirmed_with_it), "reservation_room_checked_in_at_check");
    assert_eq!(constraint(checked_in_without_it), "reservation_room_checked_in_at_check");
    assert_eq!(constraint(checked_out_without_it), "reservation_room_checked_in_at_check");
    assert_eq!(constraint(no_show_with_it), "reservation_room_checked_in_at_check");
    assert!(checked_in_with_it.is_ok(), "{checked_in_with_it:?}");
    assert!(confirmed_without_it.is_ok(), "{confirmed_without_it:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checked_in_business_date_is_set_exactly_when_checked_in_at_is(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    // checked_in_at set (satisfies the status check), business date missing: violates this check alone.
    let without_business_date = set_checkin(&pool, &hotel, "checked_in", true, false, false).await;
    // checked_in_at null (satisfies the status check for `confirmed`), business date set anyway.
    let business_date_without_it = set_checkin(&pool, &hotel, "confirmed", false, true, false).await;
    // both set together, both null together.
    let both_set = set_checkin(&pool, &hotel, "checked_in", true, true, false).await;
    let both_null = set_checkin(&pool, &hotel, "confirmed", false, false, false).await;

    assert_eq!(constraint(without_business_date), "reservation_room_checked_in_business_date_check");
    assert_eq!(constraint(business_date_without_it), "reservation_room_checked_in_business_date_check");
    assert!(both_set.is_ok(), "{both_set:?}");
    assert!(both_null.is_ok(), "{both_null:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checked_out_at_is_set_exactly_when_the_room_is_checked_out(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let checked_out_without_it = set_checkin(&pool, &hotel, "checked_out", true, true, false).await;
    let confirmed_with_it = set_checkin(&pool, &hotel, "confirmed", false, false, true).await;
    let checked_in_with_it = set_checkin(&pool, &hotel, "checked_in", true, true, true).await;
    let checked_out_with_it = set_checkin(&pool, &hotel, "checked_out", true, true, true).await;

    assert_eq!(constraint(checked_out_without_it), "reservation_room_checked_out_at_check");
    assert_eq!(constraint(confirmed_with_it), "reservation_room_checked_out_at_check");
    assert_eq!(constraint(checked_in_with_it), "reservation_room_checked_out_at_check");
    assert!(checked_out_with_it.is_ok(), "{checked_out_with_it:?}");
}
