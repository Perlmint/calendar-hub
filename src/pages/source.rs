use std::collections::BTreeMap;

use chrono::Utc;
use dioxus::prelude::*;

use super::vault::VaultKey;

#[server]
pub async fn list_sources() -> Result<BTreeMap<VaultKey, chrono::DateTime<Utc>>, ServerFnError> {
    use crate::server::prelude::{common::*, user::*};
    let session: Session = extract().await?;
    let Extension(db): Extension<SqlitePool> = extract().await?;

    let user = session.get_user().await?;
    let ret = query!(
        "SELECT
            `vault_key` as `vault_key: VaultKey`,
            `last_synced` as `last_synced: chrono::DateTime<Utc>`
        FROM `source`
        WHERE `user_id` = ?",
        user.user_id
    )
    .fetch_all(&db)
    .await
    .map_err(sqlx_error_to_dioxus_error)?
    .into_iter()
    .map(|v| (v.vault_key, v.last_synced))
    .collect();

    Ok(ret)
}

#[cfg(feature = "server")]
pub async fn update_last_synced(
    user_id: crate::server::user::UserId,
    key: VaultKey,
    db: &sqlx::SqlitePool,
) -> anyhow::Result<()> {
    let now = Utc::now();
    sqlx::query!(
        r#"UPDATE `source`
            SET `last_synced` = ?
            WHERE `user_id` = ? AND `vault_key` = ?"#,
        now,
        user_id,
        key
    )
    .execute(db)
    .await?;

    Ok(())
}
