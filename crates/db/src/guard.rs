use sqlx::PgPool;

#[derive(Debug, thiserror::Error)]
pub enum RlsBypassed {
    #[error(
        "database role `{0}` bypasses row-level security (superuser, BYPASSRLS or table owner); \
         connect as the API role instead"
    )]
    Role(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Fails unless row-level security applies to the pool's role. Superusers, `BYPASSRLS` roles and table
/// owners (directly or through role membership) skip or can disable the policies, so the API refuses to run as one.
pub async fn assert_rls_applies(pool: &PgPool) -> Result<(), RlsBypassed> {
    let (role, bypasses): (String, bool) = sqlx::query_as(
        "select r.rolname::text, r.rolsuper or r.rolbypassrls or exists (
             select 1 from pg_tables t
             where t.schemaname = 'public' and pg_has_role(current_user, t.tableowner, 'USAGE'))
         from pg_roles r where r.rolname = current_user",
    )
    .fetch_one(pool)
    .await?;
    if bypasses { Err(RlsBypassed::Role(role)) } else { Ok(()) }
}
