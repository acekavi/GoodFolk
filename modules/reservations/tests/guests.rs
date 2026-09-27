mod common;

use common::{Hotel, new_guest, with_passport};
use db::crypto::guest_aad;
use rates::Residency;
use reservations::{Guest, GuestChanges, IdDocType, NewGuest, ReservationsError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

const PASSPORT: &str = "N1234567";

fn invalid<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// Fails if any guest, as the API would serialize it, carries `secret`.
fn assert_hidden(guests: &[&Guest], secret: &str) {
    for guest in guests {
        let json = serde_json::to_string(guest).unwrap();
        assert!(!json.contains(secret), "{json} shows the ID number");
    }
}

/// The ID document columns as stored: type, sealed number, key id and last 4 characters.
async fn stored_id_doc(
    hotel: &Hotel,
    guest: Uuid,
) -> (Option<String>, Option<Vec<u8>>, Option<String>, Option<String>) {
    sqlx::query_as("select id_doc_type, id_doc_number_enc, id_doc_key_id, id_doc_last4 from guest where id = $1")
        .bind(guest)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_is_created_and_read_back_with_the_id_number_masked(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let input = NewGuest {
        email: Some(" Ada.Perera@Example.com ".into()),
        phone: Some("+94 77 123 4567".into()),
        country: Some("LK".into()),
        residency: Residency::Resident,
        notes: "Prefers a sea view".into(),
        ..with_passport(new_guest("  Ada ", " Perera  "), PASSPORT)
    };

    let created = hotel.guest(input).await;
    let read = reservations::get_guest(&mut hotel.tx().await, created.id).await.unwrap();

    assert_eq!(read, created);
    assert_eq!((read.first_name.as_str(), read.last_name.as_str()), ("Ada", "Perera"), "names are trimmed");
    assert_eq!(read.email.as_deref(), Some("ada.perera@example.com"));
    assert_eq!(read.phone.as_deref(), Some("+94 77 123 4567"));
    assert_eq!(read.country.as_deref(), Some("LK"));
    assert_eq!(read.residency, Residency::Resident);
    assert_eq!(read.id_doc_type, Some(IdDocType::Passport));
    assert_eq!(read.id_doc_masked.as_deref(), Some("•••• 4567"));
    assert_eq!(read.notes, "Prefers a sea view");
    assert_eq!(read.version, 1);
    assert_hidden(&[&created, &read], PASSPORT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_stored_number_opens_only_with_its_own_guests_aad(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = hotel.guest(with_passport(new_guest("Ada", "Perera"), PASSPORT)).await;
    let other = hotel.guest(with_passport(new_guest("Nimal", "Silva"), "X9876543")).await;

    let (kind, sealed, key_id, last4) = stored_id_doc(&hotel, ada.id).await;
    let (sealed, key_id) = (sealed.unwrap(), key_id.unwrap());

    assert_eq!(kind.as_deref(), Some("passport"));
    assert_eq!(key_id, hotel.key.id());
    assert_eq!(last4.as_deref(), Some("4567"));
    assert!(!sealed.windows(PASSPORT.len()).any(|window| window == PASSPORT.as_bytes()), "stored in plain text");
    assert_eq!(hotel.key.open(&key_id, &sealed, &guest_aad(hotel.tenant.0, ada.id)).unwrap(), PASSPORT);
    assert!(hotel.key.open(&key_id, &sealed, &guest_aad(hotel.tenant.0, other.id)).is_err());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_id_document_is_replaced_then_removed(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = hotel.guest(with_passport(new_guest("Ada", "Perera"), PASSPORT)).await;

    let nic = "199012345678";
    let changes = GuestChanges { id_doc: Some(Some((IdDocType::Nic, nic.into()))), ..GuestChanges::default() };
    let replaced = hotel.try_update_guest(&ada, changes).await.unwrap();

    assert_eq!(replaced.id_doc_type, Some(IdDocType::Nic));
    assert_eq!(replaced.id_doc_masked.as_deref(), Some("•••• 5678"));
    assert_eq!(replaced.version, 2);
    let (_, sealed, key_id, _) = stored_id_doc(&hotel, ada.id).await;
    let opened = hotel.key.open(&key_id.unwrap(), &sealed.unwrap(), &guest_aad(hotel.tenant.0, ada.id)).unwrap();
    assert_eq!(opened, nic);

    let removed = hotel.try_update_guest(&replaced, GuestChanges { id_doc: Some(None), ..GuestChanges::default() });
    let removed = removed.await.unwrap();

    assert_eq!((removed.id_doc_type, removed.id_doc_masked.as_deref()), (None, None));
    assert_eq!(stored_id_doc(&hotel, ada.id).await, (None, None, None, None));
    assert_hidden(&[&ada, &replaced, &removed], PASSPORT);
    assert_hidden(&[&replaced, &removed], nic);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_change_leaves_the_fields_it_does_not_name(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let input =
        NewGuest { email: Some("ada@example.com".into()), ..with_passport(new_guest("Ada", "Perera"), PASSPORT) };
    let ada = hotel.guest(input).await;

    let changes = GuestChanges {
        last_name: Some(" Perera-Silva ".into()),
        email: Some(None),
        country: Some(Some("GB".into())),
        ..GuestChanges::default()
    };
    let updated = hotel.try_update_guest(&ada, changes).await.unwrap();

    assert_eq!(updated.first_name, "Ada");
    assert_eq!(updated.last_name, "Perera-Silva");
    assert_eq!((updated.email, updated.country.as_deref()), (None, Some("GB")));
    assert_eq!(updated.id_doc_masked.as_deref(), Some("•••• 4567"), "the ID document is kept");
    let (_, sealed, key_id, _) = stored_id_doc(&hotel, ada.id).await;
    let opened = hotel.key.open(&key_id.unwrap(), &sealed.unwrap(), &guest_aad(hotel.tenant.0, ada.id)).unwrap();
    assert_eq!(opened, PASSPORT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_update_from_an_older_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = hotel.guest(new_guest("Ada", "Perera")).await;
    hotel.try_update_guest(&ada, GuestChanges { notes: Some("VIP".into()), ..GuestChanges::default() }).await.unwrap();

    let stale = hotel.try_update_guest(&ada, GuestChanges { notes: Some("late".into()), ..GuestChanges::default() });

    assert!(matches!(stale.await, Err(ReservationsError::VersionMismatch("guest"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guests_are_found_by_partial_or_misspelt_name_email_and_phone(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = NewGuest {
        email: Some("ada@example.com".into()),
        phone: Some("+94771234567".into()),
        ..with_passport(new_guest("Ada", "Perera"), PASSPORT)
    };
    let ada = hotel.guest(ada).await;
    let nimal = hotel.guest(new_guest("Nimal", "Silva")).await;
    let ids = |found: &[Guest]| found.iter().map(|guest| guest.id).collect::<Vec<_>>();

    assert_eq!(ids(&hotel.search("pere").await), [ada.id], "partial last name");
    assert_eq!(ids(&hotel.search("ADA").await), [ada.id], "case-insensitive first name");
    assert_eq!(ids(&hotel.search("Perrera").await), [ada.id], "a typo");
    assert_eq!(ids(&hotel.search("ada perera").await), [ada.id], "full name");
    assert_eq!(ids(&hotel.search("ADA@example.com").await), [ada.id], "exact email");
    assert_eq!(ids(&hotel.search(" +94771234567 ").await), [ada.id], "exact phone");
    assert_eq!(ids(&hotel.search("silva").await), [nimal.id]);
    assert!(hotel.search("+9477").await.is_empty(), "phone matches only exactly");
    assert!(hotel.search("Wickramasinghe").await.is_empty());
    assert_hidden(&hotel.search("ada").await.iter().collect::<Vec<_>>(), PASSPORT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn search_ranks_the_closest_name_first(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let pereira = hotel.guest(new_guest("Ann", "Pereira")).await;
    let perera = hotel.guest(new_guest("Ann", "Perera")).await;

    let found = hotel.search("Ann Perera").await;

    assert_eq!(found.iter().map(|guest| guest.id).collect::<Vec<_>>(), [perera.id, pereira.id]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_blank_search_lists_the_newest_guests_up_to_the_limit(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut created = Vec::new();
    for name in ["One", "Two", "Three"] {
        created.push(hotel.guest(new_guest("", name)).await.id);
    }

    let newest = reservations::search_guests(&mut hotel.tx().await, "  ", 2).await.unwrap();

    assert_eq!(newest.iter().map(|guest| guest.id).collect::<Vec<_>>(), [created[2], created[1]]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_guest_is_neither_found_nor_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let ours = Hotel::new(opts.clone()).await;
    let theirs = Hotel::new(opts).await;
    let guest = theirs.guest(NewGuest { email: Some("ada@example.com".into()), ..new_guest("Ada", "Perera") }).await;

    let read = reservations::get_guest(&mut ours.tx().await, guest.id).await;
    let changes = GuestChanges { notes: Some("mine".into()), ..GuestChanges::default() };
    let updated = ours.try_update_guest(&guest, changes).await;

    assert!(matches!(read, Err(ReservationsError::NotFound("guest"))));
    assert!(matches!(updated, Err(ReservationsError::NotFound("guest"))));
    assert!(ours.search("Ada Perera").await.is_empty());
    assert!(ours.search("ada@example.com").await.is_empty());
    assert!(ours.search("").await.is_empty());
    assert_eq!(theirs.search("ada").await, [guest]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_may_have_a_single_name(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    let guest = hotel.guest(new_guest("   ", "Madonna")).await;

    assert_eq!((guest.first_name.as_str(), guest.last_name.as_str()), ("", "Madonna"));
    assert_eq!(hotel.search("madona").await, [guest]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_fields_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = || new_guest("Ada", "Perera");

    assert_eq!(invalid(hotel.try_guest(new_guest("Ada", "  ")).await), "a last name is 1 to 100 characters");
    assert_eq!(
        invalid(hotel.try_guest(new_guest(&"A".repeat(101), "Perera")).await),
        "a first name is at most 100 characters"
    );
    for email in ["ada", "ada@", "@example.com", "ada@example", "ada perera@example.com", "a@b@example.com"] {
        let input = NewGuest { email: Some(email.into()), ..ada() };
        assert_eq!(invalid(hotel.try_guest(input).await), "an email looks like name@example.com", "{email}");
    }
    for country in ["lk", "LKA", "L1", ""] {
        let input = NewGuest { country: Some(country.into()), ..ada() };
        assert_eq!(invalid(hotel.try_guest(input).await), "a country is a two-letter code such as LK", "{country}");
    }
    for phone in ["12", &"1".repeat(31)] {
        let input = NewGuest { phone: Some(phone.into()), ..ada() };
        assert_eq!(invalid(hotel.try_guest(input).await), "a phone number is 3 to 30 characters", "{phone}");
    }
    let input = NewGuest { notes: "n".repeat(2001), ..ada() };
    assert_eq!(invalid(hotel.try_guest(input).await), "notes are at most 2000 characters");
    for number in [" ", &"9".repeat(51)] {
        assert_eq!(invalid(hotel.try_guest(with_passport(ada(), number)).await), "an ID number is 1 to 50 characters");
    }

    let guest = hotel.guest(ada()).await;
    let changes = GuestChanges { phone: Some(Some("1".into())), ..GuestChanges::default() };
    assert_eq!(invalid(hotel.try_update_guest(&guest, changes).await), "a phone number is 3 to 30 characters");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn audit_entries_name_the_id_document_by_type_and_last_4_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ada = hotel.guest(with_passport(new_guest("Ada", "Perera"), PASSPORT)).await;
    let changes = GuestChanges {
        email: Some(Some("ada@example.com".into())),
        id_doc: Some(Some((IdDocType::Nic, "199012345678".into()))),
        ..GuestChanges::default()
    };
    hotel.try_update_guest(&ada, changes).await.unwrap();

    let entries: Vec<(String, serde_json::Value)> =
        sqlx::query_as("select action, data from audit_log where entity = 'guest' and entity_id = $1 order by at, id")
            .bind(ada.id)
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();

    assert_eq!(
        entries,
        [
            ("guest.created".to_owned(), serde_json::json!({ "id_doc": { "type": "passport", "last4": "4567" } })),
            (
                "guest.updated".to_owned(),
                serde_json::json!({ "fields": ["email", "id_doc"], "id_doc": { "type": "nic", "last4": "5678" } })
            ),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_short_id_number_is_masked_correctly(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let short_id = "A123";
    let guest = hotel
        .guest(NewGuest { id_doc: Some((IdDocType::Other, short_id.into())), ..new_guest("Bob", "Builder") })
        .await;

    // 4 characters: reveal at most 4, never more than half (4/2=2), so last 2 chars
    assert_eq!(guest.id_doc_masked.as_deref(), Some("•••• 23"));
    assert_eq!(guest.id_doc_type, Some(IdDocType::Other));
    assert_hidden(&[&guest], short_id);

    // Check the audit log doesn't contain the full ID
    let entries: Vec<String> =
        sqlx::query_scalar("select action from audit_log where entity = 'guest' and entity_id = $1")
            .bind(guest.id)
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(entries, ["guest.created"]);
    let json: serde_json::Value =
        sqlx::query_scalar("select data from audit_log where entity = 'guest' and entity_id = $1")
            .bind(guest.id)
            .fetch_one(&mut *hotel.tx().await)
            .await
            .unwrap();
    let json_str = json.to_string();
    assert!(!json_str.contains(short_id), "{json_str}");
}

#[test]
fn debug_output_hides_the_id_number() {
    let input = with_passport(new_guest("Ada", "Perera"), PASSPORT);
    let changes = GuestChanges { id_doc: Some(Some((IdDocType::Nic, PASSPORT.into()))), ..GuestChanges::default() };

    for debug in [format!("{input:?}"), format!("{changes:?}")] {
        assert!(!debug.contains(PASSPORT), "{debug}");
        assert!(debug.contains("Ada") || debug.contains("Nic"), "{debug}");
    }
}
