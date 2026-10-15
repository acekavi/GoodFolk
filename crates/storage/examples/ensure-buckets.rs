//! Creates the development buckets on the S3 server in the environment (RustFS by default).

use storage::{Storage, StorageConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Storage::new(StorageConfig::dev()?)?.ensure_dev_buckets().await?;
    println!("buckets ready");
    Ok(())
}
