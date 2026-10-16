//! A property's tax rules as stored in `tax_rule`.

use db::Tx;
use sqlx::types::Json;
use time::Date;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct RuleRow {
    code: String,
    name: String,
    kind: String,
    rate_bp: i32,
    applies_to: Vec<String>,
    base: Vec<String>,
    sequence: i32,
    presentation: String,
    condition: Json<serde_json::Value>,
    effective_from: Date,
    effective_to: Option<Date>,
}

/// The column values are the serde names of the `tax` enums, so a row converts through JSON: an unknown kind,
/// presentation or condition fails as a decode error instead of being guessed at.
fn into_rule(row: RuleRow) -> Result<tax::Rule, sqlx::Error> {
    let decode = |err: serde_json::Error| sqlx::Error::Decode(Box::new(err));
    Ok(tax::Rule {
        code: row.code,
        name: row.name,
        kind: serde_json::from_value(row.kind.into()).map_err(decode)?,
        rate_bp: u32::try_from(row.rate_bp).map_err(|err| sqlx::Error::Decode(Box::new(err)))?,
        applies_to: row.applies_to,
        base: row.base,
        sequence: row.sequence,
        presentation: serde_json::from_value(row.presentation.into()).map_err(decode)?,
        condition: serde_json::from_value(row.condition.0).map_err(decode)?,
        effective_from: row.effective_from,
        effective_to: row.effective_to,
    })
}

/// Every rule of `property`, in calculation order. `tx` must be scoped to the property's tenant.
pub async fn load_rules(tx: &mut Tx, property: Uuid) -> Result<Vec<tax::Rule>, sqlx::Error> {
    let rows: Vec<RuleRow> = sqlx::query_as(
        "select code, name, kind, rate_bp, applies_to, base, sequence, presentation, condition, effective_from,
                effective_to
         from tax_rule where property_id = $1 order by sequence, effective_from",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter().map(into_rule).collect()
}
