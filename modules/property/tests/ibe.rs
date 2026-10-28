use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use property::{
    IbeSiteChanges, NewProperty, PropertyError, contrast_with_white, create_property, ibe_site, update_ibe_site,
};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

async fn tenant_with_user(pool: &PgPool) -> (TenantId, UserId) {
    let tenant = TenantId(Uuid::now_v7());
    let user = UserId(Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
        .bind(user.0)
        .bind(format!("{}@example.com", user.0))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (tenant, user)
}

async fn property(pool: &PgPool, tenant: TenantId, user: UserId, code: &str) -> Uuid {
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    let input = NewProperty {
        code: code.into(),
        name: format!("Hotel {code}"),
        timezone: "Asia/Colombo".into(),
        base_currency: "LKR".into(),
    };
    let created = create_property(&mut tx, tenant, user, input).await.unwrap();
    tx.commit().await.unwrap();
    created.id
}

/// A ready asset of `purpose`; a `gallery` one is also in a new room type's gallery.
async fn asset(pool: &PgPool, tenant: TenantId, property: Uuid, purpose: &str) -> Uuid {
    let (id, room_type) = (Uuid::now_v7(), Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query(
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, status, filename, declared_bytes, sha256)
         values ($1, $2, $3, $4, 'public', 'ready', 'a.jpg', 10, sha256('x'::bytea))",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(property)
    .bind(purpose)
    .execute(&mut *tx)
    .await
    .unwrap();
    if purpose == "gallery" {
        sqlx::query(
            "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children,
                                    max_occupancy)
             values ($1, $2, $3, 'DLX', 'Deluxe', 2, 2, 0, 2)",
        )
        .bind(room_type)
        .bind(tenant.0)
        .bind(property)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("insert into room_type_media (tenant_id, room_type_id, asset_id, position) values ($1, $2, $3, 0)")
            .bind(tenant.0)
            .bind(room_type)
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    id
}

fn changes() -> IbeSiteChanges {
    IbeSiteChanges {
        enabled: true,
        custom_host: Some("stay.galle.example".into()),
        logo_asset_id: None,
        hero_asset_id: None,
        accent: Some("#1d4ed8".into()),
        description: "By the sea".into(),
        contact_phone: Some("+94 91 000 0000".into()),
        contact_email: Some("hello@galle.example".into()),
        terms: "No smoking.".into(),
        privacy: "We keep your name.".into(),
    }
}

#[test]
fn white_text_contrast_follows_wcag() {
    assert!((contrast_with_white("#ffffff").unwrap() - 1.0).abs() < 1e-9);
    assert!((contrast_with_white("#000000").unwrap() - 21.0).abs() < 1e-9);
    assert!(contrast_with_white("#1d4ed8").unwrap() > 4.5);
    assert!(contrast_with_white("#ff0000").unwrap() < 4.5);
    assert_eq!(contrast_with_white("1d4ed8"), None);
    assert_eq!(contrast_with_white("#1d4ed"), None);
    assert_eq!(contrast_with_white("#1d4edz"), None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_property_that_never_saved_has_the_defaults_at_version_zero(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let galle = property(&pool, tenant, user, "GAL").await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let site = ibe_site(&mut tx, galle).await.unwrap();
    let unknown = ibe_site(&mut tx, Uuid::now_v7()).await;

    assert_eq!((site.version, site.enabled, site.custom_host), (0, false, None));
    assert!(matches!(unknown, Err(PropertyError::NotFound)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_save_round_trips_bumps_the_version_and_is_audited(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let galle = property(&pool, tenant, user, "GAL").await;
    let logo = asset(&pool, tenant, galle, "ibe_logo").await;
    let hero = asset(&pool, tenant, galle, "gallery").await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let first = update_ibe_site(&mut tx, tenant, user, galle, 0, changes()).await.unwrap();
    let with_images = IbeSiteChanges { logo_asset_id: Some(logo), hero_asset_id: Some(hero), ..changes() };
    let second = update_ibe_site(&mut tx, tenant, user, galle, 1, with_images).await.unwrap();

    assert_eq!(first.version, 1);
    assert_eq!(first.accent.as_deref(), Some("#1d4ed8"));
    assert_eq!(second.version, 2);
    assert_eq!((second.logo_asset_id, second.hero_asset_id), (Some(logo), Some(hero)));
    assert_eq!(ibe_site(&mut tx, galle).await.unwrap(), second);
    let audits: Vec<(serde_json::Value, serde_json::Value)> = sqlx::query_as(
        "select data -> 'before', data -> 'after' from audit_log where action = 'ibe_site.updated' order by id",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audits.len(), 2);
    assert!(audits[0].0.is_null());
    assert_eq!(audits[0].1["custom_host"], "stay.galle.example");
    assert_eq!(audits[1].0["logo_asset_id"], serde_json::Value::Null);
    assert_eq!(audits[1].1["logo_asset_id"], logo.to_string());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let galle = property(&pool, tenant, user, "GAL").await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    update_ibe_site(&mut tx, tenant, user, galle, 0, changes()).await.unwrap();

    let first_save_again = update_ibe_site(&mut tx, tenant, user, galle, 0, changes()).await;
    let ahead = update_ibe_site(&mut tx, tenant, user, galle, 5, changes()).await;

    assert!(matches!(first_save_again, Err(PropertyError::VersionMismatch)));
    assert!(matches!(ahead, Err(PropertyError::VersionMismatch)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_accent_without_the_contrast_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let galle = property(&pool, tenant, user, "GAL").await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    for accent in ["#ffffff", "#ffeb3b"] {
        let refused = update_ibe_site(
            &mut tx,
            tenant,
            user,
            galle,
            0,
            IbeSiteChanges { accent: Some(accent.into()), ..changes() },
        )
        .await;
        match refused {
            Err(PropertyError::Invalid(message)) => {
                assert_eq!(message, "The accent colour needs more contrast with white text", "{accent}");
            }
            other => panic!("{accent}: {other:?}"),
        }
    }
    assert!(update_ibe_site(&mut tx, tenant, user, galle, 0, changes()).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_custom_host_used_elsewhere_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let (galle, kandy) = (property(&pool, tenant, user, "GAL").await, property(&pool, tenant, user, "KAN").await);
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    update_ibe_site(&mut tx, tenant, user, galle, 0, changes()).await.unwrap();
    tx.commit().await.unwrap();

    let other_tenant = tenant_with_user(&pool).await;
    let elsewhere = property(&pool, other_tenant.0, other_tenant.1, "GAL").await;
    let mut tx = begin(&pool, Scope::tenant(other_tenant.0)).await.unwrap();
    let taken = update_ibe_site(&mut tx, other_tenant.0, other_tenant.1, elsewhere, 0, changes()).await;
    drop(tx);
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    let same_tenant = update_ibe_site(&mut tx, tenant, user, kandy, 0, changes()).await;

    assert!(matches!(taken, Err(PropertyError::Invalid(ref m)) if m.contains("already used")), "{taken:?}");
    assert!(matches!(same_tenant, Err(PropertyError::Invalid(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_hero_must_be_a_gallery_photo_of_this_property_and_the_logo_a_logo_of_it(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let (galle, kandy) = (property(&pool, tenant, user, "GAL").await, property(&pool, tenant, user, "KAN").await);
    let own_photo = asset(&pool, tenant, galle, "gallery").await;
    let other_photo = asset(&pool, tenant, kandy, "gallery").await;
    let own_logo = asset(&pool, tenant, galle, "ibe_logo").await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let save = |logo, hero| IbeSiteChanges { logo_asset_id: logo, hero_asset_id: hero, ..changes() };
    let another_property = update_ibe_site(&mut tx, tenant, user, galle, 0, save(None, Some(other_photo))).await;
    let a_logo_as_hero = update_ibe_site(&mut tx, tenant, user, galle, 0, save(None, Some(own_logo))).await;
    let a_photo_as_logo = update_ibe_site(&mut tx, tenant, user, galle, 0, save(Some(own_photo), None)).await;
    let unknown = update_ibe_site(&mut tx, tenant, user, galle, 0, save(None, Some(Uuid::now_v7()))).await;
    let fine = update_ibe_site(&mut tx, tenant, user, galle, 0, save(Some(own_logo), Some(own_photo))).await;

    for refused in [another_property, a_logo_as_hero, a_photo_as_logo, unknown] {
        assert!(matches!(refused, Err(PropertyError::Invalid(_))), "{refused:?}");
    }
    assert!(fine.is_ok());
}
