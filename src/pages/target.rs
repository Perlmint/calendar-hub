use std::collections::BTreeMap;

use chrono::Utc;
use dioxus::prelude::*;

#[derive(
    Debug,
    Clone,
    serde::Deserialize,
    serde::Serialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    enum_iterator::Sequence,
)]
pub enum TargetType {
    GoogleCalendar,
}

impl std::fmt::Display for TargetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetType::GoogleCalendar => f.write_str("GoogleCalendar"),
        }
    }
}

#[server]
pub async fn list_targets() -> Result<BTreeMap<TargetType, chrono::DateTime<Utc>>, ServerFnError> {
    use crate::server::prelude::{common::*, user::*};
    let session: Session = extract().await?;
    let Extension(db): Extension<SqlitePool> = extract().await?;

    let user = session.get_user().await?;
    let google_calendar = query!(
        "SELECT
            `calendar_id`,
            `last_synced` as `last_synced: chrono::DateTime<Utc>`
        FROM `google_user`
        WHERE `user_id` = ?",
        user.user_id
    )
    .fetch_optional(&db)
    .await
    .map_err(sqlx_error_to_dioxus_error)?
    .and_then(|r| {
        (!r.calendar_id.is_empty()).then(move || (TargetType::GoogleCalendar, r.last_synced))
    });

    Ok([google_calendar].into_iter().filter_map(|v| v).collect())
}
