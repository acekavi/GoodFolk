//! Constraints the Phase 1 migration puts on rooms and blocks, checked directly in the database.

use sqlx::PgPool;
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    room_type: Uuid,
    room: Uuid,
    reason: Uuid,
}

/// A tenant with one property, room type, room and block reason. Runs as the superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel {
        tenant: Uuid::now_v7(),
        property: Uuid::now_v7(),
        room_type: Uuid::now_v7(),
        room: Uuid::now_v7(),
        reason: Uuid::now_v7(),
    };
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(hotel.tenant).execute(pool).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, $3, 'Hotel', 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(hotel.property)
    .bind(hotel.tenant)
    .bind(code)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         values ($1, $2, $3, 'DLX', 'Deluxe', 2, 2, 0, 2)",
    )
    .bind(hotel.room_type)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("insert into room (id, tenant_id, property_id, room_type_id, number) values ($1, $2, $3, $4, '101')")
        .bind(hotel.room)
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(hotel.room_type)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
         values ($1, $2, $3, 'LEAK', 'Leak', 'out_of_order')",
    )
    .bind(hotel.reason)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .execute(pool)
    .await
    .unwrap();
    hotel
}

/// Blocks `hotel`'s room for `[today + from, today + to)`.
async fn block(pool: &PgPool, hotel: &Hotel, from: i32, to: i32) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id)
         values ($1, $2, $3, $4, daterange(current_date + $5, current_date + $6), 'out_of_order', $7)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.room)
    .bind(from)
    .bind(to)
    .bind(hotel.reason)
    .execute(pool)
    .await?;
    Ok(id)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn overlapping_active_blocks_on_one_room_are_rejected(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let first = block(&pool, &hotel, 0, 5).await.unwrap();

    let overlapping = block(&pool, &hotel, 4, 6).await.unwrap_err();
    let adjacent = block(&pool, &hotel, 5, 7).await;
    sqlx::query("update room_block set released_at = now() where id = $1").bind(first).execute(&pool).await.unwrap();
    let after_release = block(&pool, &hotel, 2, 4).await;

    let constraint = overlapping.as_database_error().and_then(|e| e.constraint()).map(str::to_owned);
    assert_eq!(constraint.as_deref(), Some("room_block_no_overlap"));
    assert!(adjacent.is_ok(), "[5, 7) touches [0, 5) without overlapping: {adjacent:?}");
    assert!(after_release.is_ok(), "{after_release:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_cannot_use_another_propertys_room_type(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    let result = sqlx::query(
        "insert into room (id, tenant_id, property_id, room_type_id, number) values ($1, $2, $3, $4, '102')",
    )
    .bind(Uuid::now_v7())
    .bind(galle.tenant)
    .bind(galle.property)
    .bind(kandy.room_type)
    .execute(&pool)
    .await;

    let err = result.unwrap_err();
    assert_eq!(err.as_database_error().and_then(|e| e.code()).as_deref(), Some("23503"), "{err}");
}

#[sqlx::test(migrations = false)]
async fn existing_properties_get_a_business_date_and_block_reasons(pool: PgPool) {
    db::MIGRATOR.run_to(3, &pool).await.unwrap();
    let tenant = Uuid::now_v7();
    let property = Uuid::now_v7();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant).execute(&pool).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency)
         values ($1, $2, 'GAL', 'Galle', 'Pacific/Kiritimati', 'LKR')",
    )
    .bind(property)
    .bind(tenant)
    .execute(&pool)
    .await
    .unwrap();

    db::MIGRATOR.run(&pool).await.unwrap();

    let (business_date_is_local_today, check_in, check_out): (bool, String, String) = sqlx::query_as(
        "select business_date = (now() at time zone 'Pacific/Kiritimati')::date,
                to_char(check_in_time, 'HH24:MI'), to_char(check_out_time, 'HH24:MI')
         from property where id = $1",
    )
    .bind(property)
    .fetch_one(&pool)
    .await
    .unwrap();
    let reasons: Vec<(String, String)> =
        sqlx::query_as("select code, default_kind from block_reason where property_id = $1 order by code")
            .bind(property)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(business_date_is_local_today);
    assert_eq!((check_in.as_str(), check_out.as_str()), ("14:00", "12:00"));
    assert_eq!(
        reasons,
        [
            ("CONSTRUCTION", "out_of_order"),
            ("DEEP_CLEAN", "out_of_service"),
            ("MAINTENANCE", "out_of_order"),
            ("OTHER", "out_of_order"),
            ("RENOVATION", "out_of_order"),
        ]
        .map(|(code, kind)| (code.to_owned(), kind.to_owned()))
    );
}
