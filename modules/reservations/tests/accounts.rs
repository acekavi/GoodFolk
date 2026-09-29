mod common;

use common::{Hotel, new_account, new_guest};
use reservations::{
    Account, AccountChanges, AccountContact, AccountKind, MAX_ACCOUNT_LIST, NewAccount, ReservationChanges,
    ReservationsError,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn invalid<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_is_created_and_read_back(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let input = NewAccount {
        kind: AccountKind::TravelAgent,
        name: "  Ceylon Travels  ".into(),
        contact: AccountContact {
            email: Some(" Bookings@Ceylon.example ".into()),
            phone: Some(" +94 11 234 5678 ".into()),
            address: Some(" 12 Galle Road, Colombo ".into()),
            contact_name: Some(" Kamal Perera ".into()),
        },
        credit_limit: Some(500_000),
        currency: " USD ".into(),
    };

    let created = hotel.account(input).await;
    let read = reservations::get_account(&mut hotel.tx().await, created.id).await.unwrap();

    assert_eq!(read, created);
    assert_eq!(read.kind, AccountKind::TravelAgent);
    assert_eq!(read.name, "Ceylon Travels", "the name is trimmed");
    assert_eq!(read.contact.email.as_deref(), Some("bookings@ceylon.example"), "the email is trimmed and lowercased");
    assert_eq!(read.contact.phone.as_deref(), Some("+94 11 234 5678"));
    assert_eq!(read.contact.address.as_deref(), Some("12 Galle Road, Colombo"));
    assert_eq!(read.contact.contact_name.as_deref(), Some("Kamal Perera"));
    assert_eq!(read.credit_limit, Some(500_000));
    assert_eq!(read.currency, "USD");
    assert!(read.active, "new accounts start active");
    assert_eq!(read.version, 1);

    let entries: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "select action, data from audit_log where entity = 'account' and entity_id = $1 order by at, id",
    )
    .bind(created.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(
        entries,
        [("account.created".to_owned(), serde_json::json!({ "kind": "travel_agent", "name": "Ceylon Travels" }))]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_is_updated_field_by_field_and_can_be_deactivated(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let account = hotel
        .account(NewAccount {
            contact: AccountContact { email: Some("old@example.com".into()), ..Default::default() },
            ..new_account("Old Name")
        })
        .await;

    let changes = AccountChanges {
        name: Some("New Name".into()),
        phone: Some(Some("+94771234567".into())),
        credit_limit: Some(Some(1_000)),
        ..Default::default()
    };
    let updated = hotel.try_update_account(&account, changes).await.unwrap();

    assert_eq!(updated.name, "New Name");
    assert_eq!(updated.contact.email.as_deref(), Some("old@example.com"), "unnamed fields are kept");
    assert_eq!(updated.contact.phone.as_deref(), Some("+94771234567"));
    assert_eq!(updated.credit_limit, Some(1_000));
    assert_eq!(updated.version, 2);

    let cleared = hotel
        .try_update_account(&updated, AccountChanges { credit_limit: Some(None), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(cleared.credit_limit, None, "a nullable field is cleared by Some(None)");

    let deactivated =
        hotel.try_update_account(&cleared, AccountChanges { active: Some(false), ..Default::default() }).await.unwrap();
    assert!(!deactivated.active);

    let entries: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "select action, data from audit_log where entity = 'account' and entity_id = $1 and action = 'account.updated'
         order by at, id",
    )
    .bind(account.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(
        entries,
        [
            ("account.updated".to_owned(), serde_json::json!({ "fields": ["name", "phone", "credit_limit"] })),
            ("account.updated".to_owned(), serde_json::json!({ "fields": ["credit_limit"] })),
            ("account.updated".to_owned(), serde_json::json!({ "fields": ["active"] })),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_update_from_an_older_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let account = hotel.account(new_account("Ceylon Travels")).await;
    hotel
        .try_update_account(&account, AccountChanges { name: Some("Renamed".into()), ..Default::default() })
        .await
        .unwrap();

    let stale = hotel.try_update_account(&account, AccountChanges { name: Some("Late".into()), ..Default::default() });

    assert!(matches!(stale.await, Err(ReservationsError::VersionMismatch("account"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn accounts_are_listed_by_name_filtered_and_searched(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let alpha = hotel.account(new_account("Alpha Tours")).await;
    let beta = hotel.account(new_account("Beta Corp")).await;
    let gamma = hotel.account(new_account("Gamma Tours")).await;
    let deactivated =
        hotel.try_update_account(&gamma, AccountChanges { active: Some(false), ..Default::default() }).await.unwrap();

    let all_active = reservations::list_accounts(&mut hotel.tx().await, "", false, 20).await.unwrap();
    assert_eq!(
        all_active.iter().map(|a| a.id).collect::<Vec<_>>(),
        [alpha.id, beta.id],
        "ordered by name, active only"
    );

    let with_inactive = reservations::list_accounts(&mut hotel.tx().await, "", true, 20).await.unwrap();
    assert_eq!(with_inactive.iter().map(|a| a.id).collect::<Vec<_>>(), [alpha.id, beta.id, deactivated.id]);

    let searched = reservations::list_accounts(&mut hotel.tx().await, "tours", true, 20).await.unwrap();
    assert_eq!(searched.iter().map(|a| a.id).collect::<Vec<_>>(), [alpha.id, deactivated.id], "case-insensitive");

    let searched_active_only = reservations::list_accounts(&mut hotel.tx().await, "TOURS", false, 20).await.unwrap();
    assert_eq!(searched_active_only.iter().map(|a| a.id).collect::<Vec<_>>(), [alpha.id]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_accounts_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    assert_eq!(
        invalid(hotel.try_account(NewAccount { name: "  ".into(), ..new_account("x") }).await),
        "a name is 1 to 200 characters"
    );
    assert_eq!(
        invalid(hotel.try_account(NewAccount { name: "n".repeat(201), ..new_account("x") }).await),
        "a name is 1 to 200 characters"
    );
    let with_email = |email: &str| NewAccount {
        contact: AccountContact { email: Some(email.into()), ..Default::default() },
        ..new_account("Acme")
    };
    assert_eq!(invalid(hotel.try_account(with_email("not-an-email")).await), "an email looks like name@example.com");
    let with_phone = |phone: &str| NewAccount {
        contact: AccountContact { phone: Some(phone.into()), ..Default::default() },
        ..new_account("Acme")
    };
    assert_eq!(invalid(hotel.try_account(with_phone("1")).await), "a phone number is 3 to 30 characters");
    let with_address = |address: &str| NewAccount {
        contact: AccountContact { address: Some(address.into()), ..Default::default() },
        ..new_account("Acme")
    };
    assert_eq!(
        invalid(hotel.try_account(with_address(&"a".repeat(501))).await),
        "an address is at most 500 characters"
    );
    let with_contact_name = |name: &str| NewAccount {
        contact: AccountContact { contact_name: Some(name.into()), ..Default::default() },
        ..new_account("Acme")
    };
    assert_eq!(
        invalid(hotel.try_account(with_contact_name(&"a".repeat(201))).await),
        "a contact name is at most 200 characters"
    );
    for limit in [-1, 100_000_000_001] {
        let input = NewAccount { credit_limit: Some(limit), ..new_account("Acme") };
        assert_eq!(invalid(hotel.try_account(input).await), "a credit limit is 0 to 100,000,000,000");
    }
    for currency in ["us", "USDD", "usd", "123"] {
        let input = NewAccount { currency: currency.into(), ..new_account("Acme") };
        assert_eq!(
            invalid(hotel.try_account(input).await),
            "a currency is a three-letter uppercase code such as USD",
            "{currency}"
        );
    }

    let account = hotel.account(new_account("Acme")).await;
    let changes = AccountChanges { phone: Some(Some("1".into())), ..Default::default() };
    assert_eq!(invalid(hotel.try_update_account(&account, changes).await), "a phone number is 3 to 30 characters");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_account_is_neither_found_nor_listed(_: PgPoolOptions, opts: PgConnectOptions) {
    let ours = Hotel::new(opts.clone()).await;
    let theirs = Hotel::new(opts).await;
    let account = theirs.account(new_account("Their Account")).await;

    let read = reservations::get_account(&mut ours.tx().await, account.id).await;
    let changes = AccountChanges { name: Some("mine".into()), ..Default::default() };
    let updated = ours.try_update_account(&account, changes).await;
    let listed = reservations::list_accounts(&mut ours.tx().await, "", true, 20).await.unwrap();

    assert!(matches!(read, Err(ReservationsError::NotFound("account"))));
    assert!(matches!(updated, Err(ReservationsError::NotFound("account"))));
    assert!(listed.is_empty());
    assert_eq!(
        reservations::list_accounts(&mut theirs.tx().await, "", true, 20).await.unwrap(),
        [account],
        "the owning tenant still sees it"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservation_can_be_billed_to_an_active_account_but_not_an_inactive_or_unknown_one(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let account = hotel.account(new_account("Ceylon Travels")).await;
    let room = hotel.room(hotel.deluxe.id, &plans.bar, 2, 4);

    let booked = hotel.try_book_for_account(&booker, Some(account.id), vec![room.clone()]).await.unwrap();
    let detail = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert_eq!(detail.account.as_ref().map(|a| a.id), Some(account.id));

    let unknown = hotel.try_book_for_account(&booker, Some(Uuid::now_v7()), vec![room.clone()]).await;
    assert_eq!(invalid(unknown), "no such account");

    let deactivated: Account =
        hotel.try_update_account(&account, AccountChanges { active: Some(false), ..Default::default() }).await.unwrap();
    let refused = hotel.try_book_for_account(&booker, Some(deactivated.id), vec![room]).await;
    assert_eq!(invalid(refused), "Ceylon Travels is no longer active");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_account_can_be_set_and_cleared_on_a_reservation_and_a_stale_version_is_refused(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let account = hotel.account(new_account("Ceylon Travels")).await;
    let other_account = hotel.account(new_account("Other Corp")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)]).await.unwrap();

    let mut tx = hotel.tx().await;
    let committed = reservations::update_reservation(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        booked.id,
        booked.version,
        ReservationChanges { account_id: Some(Some(account.id)), notes: Some("billed to the agent".into()) },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(committed.account_id, Some(account.id));
    assert_eq!(committed.notes, "billed to the agent");
    assert_eq!(committed.version, booked.version + 1);

    let stale = reservations::update_reservation(
        &mut hotel.tx().await,
        hotel.tenant,
        hotel.user,
        hotel.property,
        booked.id,
        booked.version,
        ReservationChanges { account_id: Some(Some(other_account.id)), notes: None },
    )
    .await;
    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation"))));

    let mut tx = hotel.tx().await;
    let cleared = reservations::update_reservation(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        booked.id,
        committed.version,
        ReservationChanges { account_id: Some(None), notes: None },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(cleared.account_id, None, "Some(None) clears the account");
    assert_eq!(cleared.notes, "billed to the agent", "notes is untouched when the change names only account_id");

    let mut tx = hotel.tx().await;
    let renoted = reservations::update_reservation(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        booked.id,
        cleared.version,
        ReservationChanges { account_id: None, notes: Some("front desk note".into()) },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(renoted.account_id, None, "account_id is untouched when the change names only notes");
    assert_eq!(renoted.notes, "front desk note");

    let detail = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert!(detail.account.is_none());

    let audited: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "select action, data from audit_log where entity = 'reservation' and entity_id = $1
         and action = 'reservation.updated' order by at, id",
    )
    .bind(booked.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(
        audited,
        [
            // account and notes together: account_id appears with its new value.
            (
                "reservation.updated".to_owned(),
                serde_json::json!({ "fields": ["account_id", "notes"], "account_id": account.id }),
            ),
            // account-only change: account_id appears, cleared to null.
            ("reservation.updated".to_owned(), serde_json::json!({ "fields": ["account_id"], "account_id": null })),
            // notes-only change: account_id was not touched, so it is left out entirely.
            ("reservation.updated".to_owned(), serde_json::json!({ "fields": ["notes"] })),
        ],
        "the failed stale attempt left no trace, and account_id appears only when the change touched it"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_detail_shows_the_billed_account_with_its_kind_and_name(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let account = hotel.account(new_account("Ceylon Travels")).await;
    let booked = hotel
        .try_book_for_account(&booker, Some(account.id), vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)])
        .await
        .unwrap();

    let with_account = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    let account_ref = with_account.account.expect("the reservation is billed to an account");
    assert_eq!(
        (account_ref.id, account_ref.name.as_str(), account_ref.kind),
        (account.id, "Ceylon Travels", AccountKind::Company)
    );

    let no_account = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 5, 7)]).await.unwrap();
    let without_account =
        reservations::get_reservation(&mut hotel.tx().await, hotel.property, no_account.id).await.unwrap();
    assert!(without_account.account.is_none());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_zero_limit_still_returns_one_and_a_limit_above_the_cap_is_clamped_to_it(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    // One statement, so seeding MAX_ACCOUNT_LIST + 1 rows stays fast.
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into account (id, tenant_id, kind, name, contact, currency)
         select gen_random_uuid(), $1, 'company', 'Account ' || i, '{}'::jsonb, 'USD'
         from generate_series(1, $2) as i",
    )
    .bind(hotel.tenant.0)
    .bind(MAX_ACCOUNT_LIST + 1)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let zero_limit = reservations::list_accounts(&mut hotel.tx().await, "", false, 0).await.unwrap();
    assert_eq!(zero_limit.len(), 1, "a limit of 0 still returns at least 1");

    let over_cap = reservations::list_accounts(&mut hotel.tx().await, "", false, MAX_ACCOUNT_LIST + 100).await.unwrap();
    assert_eq!(over_cap.len() as i64, MAX_ACCOUNT_LIST, "a limit above the cap returns at most the cap");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_real_account_of_another_tenant_is_no_such_account_to_create_and_update(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let theirs = Hotel::new(opts).await;
    let their_account = theirs.account(new_account("Their Account")).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;

    let create_refused = hotel
        .try_book_for_account(&booker, Some(their_account.id), vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)])
        .await;
    assert_eq!(invalid(create_refused), "no such account");

    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)]).await.unwrap();
    let update_refused = reservations::update_reservation(
        &mut hotel.tx().await,
        hotel.tenant,
        hotel.user,
        hotel.property,
        booked.id,
        booked.version,
        ReservationChanges { account_id: Some(Some(their_account.id)), notes: None },
    )
    .await;
    assert_eq!(invalid(update_refused), "no such account");
}
