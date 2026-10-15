use db::Tx;
use uuid::Uuid;

/// The Postgres channel notified when an enqueued row commits. Whoever holds a LISTEN connection wakes the relay on
/// it (see [`crate::run_relay`]).
pub const OUTBOX_CHANNEL: &str = "gf_outbox";

/// Queues `payload` for `topic` in the caller's transaction, so the event exists exactly when the business change
/// does, and notifies [`OUTBOX_CHANNEL`] when that transaction commits. The row's id (a UUID v7) is the message id
/// receivers dedupe on.
pub async fn enqueue(tx: &mut Tx, topic: &str, payload: &serde_json::Value) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("select app.outbox_enqueue($1, $2, $3)").bind(id).bind(topic).bind(payload).execute(&mut **tx).await?;
    Ok(id)
}

/// True the first time `message_id` is seen. Call it in the receiver's transaction: a rollback forgets the id, so a
/// redelivery after a failure is processed again.
pub async fn first_seen(tx: &mut Tx, message_id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("select app.event_first_seen($1)").bind(message_id).fetch_one(&mut **tx).await
}
