use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use property::{
    NewProperty, PropertyChanges, PropertyError, TaxSettingsChanges, VatReturnPeriod, create_property, list_properties,
    tax_settings, update_property, update_tax_settings,
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

fn hotel(code: &str, timezone: &str) -> NewProperty {
    NewProperty {
        code: code.into(),
        name: format!("Hotel {code}"),
        timezone: timezone.into(),
        base_currency: "LKR".into(),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn created_properties_are_listed_by_code(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let galle = create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await.unwrap();
    let kandy = create_property(&mut tx, tenant, user, hotel("KAN", "Asia/Colombo")).await.unwrap();
    let all = list_properties(&mut tx, None).await.unwrap();
    let only_kandy = list_properties(&mut tx, Some(&[kandy.id])).await.unwrap();

    assert_eq!(all, vec![galle, kandy.clone()]);
    assert_eq!(only_kandy, vec![kandy]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn codes_are_unique_within_a_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await.unwrap();

    let again = create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await;

    assert!(matches!(again, Err(PropertyError::CodeTaken)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn unknown_time_zones_are_rejected(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let result = create_property(&mut tx, tenant, user, hotel("GAL", "Mars/Olympus")).await;

    assert!(matches!(result, Err(PropertyError::UnknownTimezone)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_name_and_the_sltda_licence_round_trip(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    let galle = create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await.unwrap();
    assert_eq!(tax_settings(&mut tx, galle.id).await.unwrap().sltda_licence, None);

    let renamed = PropertyChanges { name: Some("Galle Fort Hotel".into()), ..Default::default() };
    let renamed = update_property(&mut tx, tenant, user, galle.id, galle.version, renamed).await.unwrap();
    assert_eq!(renamed.name, "Galle Fort Hotel");
    assert_eq!(list_properties(&mut tx, None).await.unwrap(), vec![renamed.clone()]);

    let set = TaxSettingsChanges { sltda_licence: Some(Some("SLTDA/GAL/0042".into())), ..Default::default() };
    let set = update_tax_settings(&mut tx, tenant, user, galle.id, renamed.version, set).await.unwrap();
    assert_eq!(set.sltda_licence.as_deref(), Some("SLTDA/GAL/0042"));
    assert_eq!(tax_settings(&mut tx, galle.id).await.unwrap(), set);

    let untouched = TaxSettingsChanges { vat_registered: Some(true), ..Default::default() };
    let untouched = update_tax_settings(&mut tx, tenant, user, galle.id, set.version, untouched).await.unwrap();
    assert_eq!(untouched.sltda_licence.as_deref(), Some("SLTDA/GAL/0042"), "left out keeps it");

    let cleared = TaxSettingsChanges { sltda_licence: Some(None), ..Default::default() };
    let cleared = update_tax_settings(&mut tx, tenant, user, galle.id, untouched.version, cleared).await.unwrap();
    assert_eq!(cleared.sltda_licence, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_tdl_registration_number_and_vat_return_period_round_trip_and_are_audited(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    let galle = create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await.unwrap();
    let initial = tax_settings(&mut tx, galle.id).await.unwrap();
    assert_eq!((initial.tdl_registration_no.as_deref(), initial.vat_return_period), (None, VatReturnPeriod::Quarterly));

    let set = TaxSettingsChanges {
        tdl_registration_no: Some(Some("TDL/GAL/0007".into())),
        vat_return_period: Some(VatReturnPeriod::Monthly),
        ..Default::default()
    };
    let set = update_tax_settings(&mut tx, tenant, user, galle.id, galle.version, set).await.unwrap();
    assert_eq!(set.tdl_registration_no.as_deref(), Some("TDL/GAL/0007"));
    assert_eq!(set.vat_return_period, VatReturnPeriod::Monthly);
    assert_eq!(tax_settings(&mut tx, galle.id).await.unwrap(), set);

    let untouched = TaxSettingsChanges { vat_registered: Some(true), ..Default::default() };
    let untouched = update_tax_settings(&mut tx, tenant, user, galle.id, set.version, untouched).await.unwrap();
    assert_eq!(untouched.tdl_registration_no.as_deref(), Some("TDL/GAL/0007"), "left out keeps it");
    assert_eq!(untouched.vat_return_period, VatReturnPeriod::Monthly);

    let cleared = TaxSettingsChanges { tdl_registration_no: Some(None), ..Default::default() };
    let cleared = update_tax_settings(&mut tx, tenant, user, galle.id, untouched.version, cleared).await.unwrap();
    assert_eq!(cleared.tdl_registration_no, None);

    let audited: Vec<serde_json::Value> = sqlx::query_scalar(
        "select data from audit_log where action = 'property.tax_settings_updated' and entity_id = $1
         order by id",
    )
    .bind(galle.id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audited[0]["before"]["tdl_registration_no"], serde_json::Value::Null);
    assert_eq!(audited[0]["before"]["vat_return_period"], "quarterly");
    assert_eq!(audited[0]["after"]["tdl_registration_no"], "TDL/GAL/0007");
    assert_eq!(audited[0]["after"]["vat_return_period"], "monthly");
}
