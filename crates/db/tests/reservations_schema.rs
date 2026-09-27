//! Constraints the Phase 3 migration puts on guests, reservations and their rooms, checked directly in the
//! database.

use sqlx::PgPool;
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    room_type: Uuid,
    room: Uuid,
    plan: Uuid,
    guest: Uuid,
    reservation: Uuid,
}

/// A tenant with one property, room type, room, rate plan, guest and reservation (`<code>-000001`). Runs as the
/// superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel {
        tenant: Uuid::now_v7(),
        property: Uuid::now_v7(),
        room_type: Uuid::now_v7(),
        room: Uuid::now_v7(),
        plan: Uuid::now_v7(),
        guest: Uuid::now_v7(),
        reservation: Uuid::now_v7(),
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
            .execute(pool)
            .await
            .unwrap();
    }
    hotel
}

/// Books `hotel`'s room type for `[today + from, today + to)` on its reservation, in `room` (or unassigned). A
/// cancelled stay is cancelled now with no penalty.
async fn stay(
    pool: &PgPool,
    hotel: &Hotel,
    room: Option<Uuid>,
    from: i32,
    to: i32,
    status: &str,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay, adults,
                                       children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
                                       cancelled_at, cancellation_penalty)
         values ($1, $2, $3, $4, $5, $6, daterange(current_date + $7, current_date + $8), 2, 0, $9, 'RO', $10, $11,
                 'USD', case when $10 = 'cancelled' then now() end, case when $10 = 'cancelled' then 0 end)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.reservation)
    .bind(hotel.room_type)
    .bind(room)
    .bind(from)
    .bind(to)
    .bind(hotel.plan)
    .bind(status)
    .bind(hotel.guest)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Inserts a reservation with confirmation number `number` for `hotel`'s guest.
async fn reservation(pool: &PgPool, hotel: &Hotel, number: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
         values ($1, $2, $3, $4, 'phone', $5)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(number)
    .bind(hotel.guest)
    .execute(pool)
    .await?;
    Ok(id)
}

fn constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|db_err| db_err.constraint()).unwrap_or_default().to_owned()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn one_room_cannot_be_assigned_to_overlapping_active_stays(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "confirmed").await.unwrap();
    let later = stay(&pool, &hotel, Some(hotel.room), 5, 7, "tentative").await.unwrap();

    let overlapping = stay(&pool, &hotel, Some(hotel.room), 2, 4, "checked_in").await;
    let moved_onto_it =
        sqlx::query("update reservation_room set stay = daterange(current_date + 1, current_date + 6) where id = $1")
            .bind(later)
            .execute(&pool)
            .await;

    assert_eq!(constraint(overlapping), "reservation_room_no_double_booking");
    assert_eq!(constraint(moved_onto_it), "reservation_room_no_double_booking");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_may_start_on_the_day_the_previous_one_checks_out(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "checked_in").await.unwrap();

    let back_to_back = stay(&pool, &hotel, Some(hotel.room), 3, 5, "confirmed").await;

    assert!(back_to_back.is_ok(), "[3, 5) touches [0, 3) without overlapping: {back_to_back:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelled_and_no_show_stays_do_not_hold_the_room(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "cancelled").await.unwrap();
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "no_show").await.unwrap();

    let active = stay(&pool, &hotel, Some(hotel.room), 1, 2, "confirmed").await;

    assert!(active.is_ok(), "{active:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn unassigned_stays_never_conflict(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    stay(&pool, &hotel, Some(hotel.room), 0, 3, "confirmed").await.unwrap();

    let first = stay(&pool, &hotel, None, 0, 3, "confirmed").await;
    let second = stay(&pool, &hotel, None, 1, 2, "confirmed").await;

    assert!(first.is_ok() && second.is_ok(), "{first:?} {second:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_cannot_use_another_propertys_room(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    let result = stay(&pool, &galle, Some(kandy.room), 0, 2, "confirmed").await;

    assert_eq!(constraint(result), "reservation_room_property_id_room_id_fkey");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_is_a_non_empty_bounded_range(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let empty = stay(&pool, &hotel, None, 2, 2, "confirmed").await;
    let open_ended = sqlx::query("update reservation_room set stay = daterange(current_date, null) where id = $1")
        .bind(stay(&pool, &hotel, None, 0, 1, "confirmed").await.unwrap())
        .execute(&pool)
        .await;

    assert_eq!(constraint(empty), "reservation_room_stay_check");
    assert_eq!(constraint(open_ended), "reservation_room_stay_check");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_records_its_cancellation_and_penalty_exactly_when_cancelled(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let room = stay(&pool, &hotel, None, 0, 2, "confirmed").await.unwrap();
    let set = |status: &'static str, cancelled: bool, penalty: Option<i64>| {
        sqlx::query(
            "update reservation_room
             set status = $2, cancelled_at = case when $3 then now() end, cancellation_penalty = $4
             where id = $1",
        )
        .bind(room)
        .bind(status)
        .bind(cancelled)
        .bind(penalty)
        .execute(&pool)
    };

    let without_cancelled_at = set("cancelled", false, None).await;
    let without_penalty = set("cancelled", true, None).await;
    let penalty_only = set("cancelled", false, Some(0)).await;
    let confirmed_with_cancelled_at = set("confirmed", true, Some(0)).await;
    let negative = set("cancelled", true, Some(-1)).await;
    let cancelled = set("cancelled", true, Some(12_000)).await;

    assert_eq!(constraint(without_cancelled_at), "reservation_room_cancellation_check");
    assert_eq!(constraint(without_penalty), "reservation_room_cancellation_check");
    assert_eq!(constraint(penalty_only), "reservation_room_cancellation_check");
    assert_eq!(constraint(confirmed_with_cancelled_at), "reservation_room_cancellation_check");
    assert_eq!(constraint(negative), "reservation_room_cancellation_penalty_check");
    assert!(cancelled.is_ok(), "{cancelled:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_id_document_is_stored_whole_or_not_at_all(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let guest = |doc_type: Option<&'static str>,
                 number: Option<Vec<u8>>,
                 key_id: Option<&'static str>,
                 last4: Option<&'static str>| {
        sqlx::query(
            "insert into guest (id, tenant_id, first_name, last_name, residency, id_doc_type, id_doc_number_enc,
                                id_doc_key_id, id_doc_last4)
             values ($1, $2, 'Ada', 'Silva', 'non_resident', $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(doc_type)
        .bind(number)
        .bind(key_id)
        .bind(last4)
        .execute(&pool)
    };
    let sealed = || Some(vec![7_u8; 40]);

    let type_only = guest(Some("passport"), None, None, None).await;
    let without_key = guest(Some("passport"), sealed(), None, Some("4567")).await;
    let without_last4 = guest(Some("nic"), sealed(), Some("k1"), None).await;
    let whole = guest(Some("passport"), sealed(), Some("k1"), Some("4567")).await;
    let none = guest(None, None, None, None).await;

    assert_eq!(constraint(type_only), "guest_id_doc_check");
    assert_eq!(constraint(without_key), "guest_id_doc_check");
    assert_eq!(constraint(without_last4), "guest_id_doc_check");
    assert!(whole.is_ok() && none.is_ok(), "{whole:?} {none:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_may_have_a_single_name(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let guest = |first_name: &'static str, last_name: &'static str| {
        sqlx::query(
            "insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, $3, $4, 'resident')",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(first_name)
        .bind(last_name)
        .execute(&pool)
    };

    let single_name = guest("", "Suharto").await;
    let no_last_name = guest("Ada", "").await;
    let found: Vec<String> = sqlx::query_scalar(
        "select last_name from guest where lower(first_name || ' ' || last_name) % 'suharto' order by last_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(single_name.is_ok(), "{single_name:?}");
    assert_eq!(constraint(no_last_name), "guest_last_name_check");
    assert_eq!(found, ["Suharto"], "the name search finds a single-name guest");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn confirmation_prefixes_have_a_pattern_index(pool: PgPool) {
    let definition: String = sqlx::query_scalar(
        "select indexdef from pg_indexes where tablename = 'reservation' and indexname = 'reservation_confirmation_prefix_idx'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(definition.contains("(property_id, confirmation_no text_pattern_ops)"), "{definition}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn confirmation_numbers_are_unique_per_property(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    let repeated = reservation(&pool, &galle, "GAL-000001").await;
    let elsewhere = reservation(&pool, &kandy, "GAL-000001").await;
    let next = reservation(&pool, &galle, "GAL-000002").await;
    let past_six_digits = reservation(&pool, &galle, "GAL-1000000").await;

    assert_eq!(constraint(repeated), "reservation_property_id_confirmation_no_key");
    assert!(elsewhere.is_ok() && next.is_ok() && past_six_digits.is_ok(), "{elsewhere:?} {next:?} {past_six_digits:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_confirmation_number_is_a_code_and_a_zero_padded_sequence(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    for number in ["GAL-12345", "GAL-0000012", "gal-000012", "GAL000012", "GAL-00001a", "G-000012"] {
        let result = reservation(&pool, &hotel, number).await;
        assert_eq!(constraint(result), "reservation_confirmation_no_check", "{number}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guest_names_have_a_trigram_index(pool: PgPool) {
    let definition: String = sqlx::query_scalar(
        "select indexdef from pg_indexes where tablename = 'guest' and indexname = 'guest_name_trgm_idx'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(definition.contains("USING gin"), "{definition}");
    assert!(definition.contains("gin_trgm_ops"), "{definition}");
    assert!(definition.contains("lower(((first_name || ' '::text) || last_name))"), "{definition}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guest_emails_must_be_lowercased(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let uppercase = sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency, email)
         values ($1, $2, 'Ada', 'Silva', 'resident', 'Ada@Example.Com')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant)
    .execute(&pool)
    .await;

    let lowercase = sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency, email)
         values ($1, $2, 'Eve', 'Perera', 'resident', 'eve@example.com')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant)
    .execute(&pool)
    .await;

    assert!(uppercase.is_err(), "uppercase email should be rejected");
    assert!(lowercase.is_ok(), "lowercase email should be accepted");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn id_document_encryption_must_be_minimum_length(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let too_short = sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency, id_doc_type, id_doc_number_enc,
                            id_doc_key_id, id_doc_last4)
         values ($1, $2, 'Ada', 'Silva', 'resident', 'passport', $3, 'k1', '4567')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant)
    .bind(vec![7_u8; 10])
    .execute(&pool)
    .await;

    let sufficient = sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency, id_doc_type, id_doc_number_enc,
                            id_doc_key_id, id_doc_last4)
         values ($1, $2, 'Eve', 'Perera', 'resident', 'nic', $3, 'k1', '8901')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant)
    .bind(vec![7_u8; 40])
    .execute(&pool)
    .await;

    assert!(too_short.is_err(), "10-byte encryption should be rejected");
    assert!(sufficient.is_ok(), "40-byte encryption should be accepted");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelled_by_cannot_be_set_on_non_cancelled_stays(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let room = stay(&pool, &hotel, None, 0, 2, "confirmed").await.unwrap();

    let set_cancelled_by = sqlx::query("update reservation_room set cancelled_by = $2 where id = $1")
        .bind(room)
        .bind(Uuid::now_v7())
        .execute(&pool)
        .await;

    assert_eq!(constraint(set_cancelled_by), "reservation_room_cancellation_check");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_must_have_a_bounded_lower_bound(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let unbounded_lower = sqlx::query(
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                       children, rate_plan_id, meal_plan, status, primary_guest_id, currency)
         values ($1, $2, $3, $4, $5, daterange(null, current_date + 5), 2, 0, $6, 'RO', 'confirmed', $7, 'USD')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.reservation)
    .bind(hotel.room_type)
    .bind(hotel.plan)
    .bind(hotel.guest)
    .execute(&pool)
    .await;

    assert_eq!(constraint(unbounded_lower), "reservation_room_stay_check");
}
