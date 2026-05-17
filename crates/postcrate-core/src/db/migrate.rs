//! Apply embedded SQL migrations at boot.

use sqlx::SqlitePool;

use crate::error::Result;

/// Compile-time-embedded migrator over `src/db/migrations/`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("src/db/migrations");

pub async fn run(pool: &SqlitePool) -> Result<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}
