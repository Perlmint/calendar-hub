use anyhow::Context;
use axum::{async_trait, Router};
#[allow(unused_imports)]
use chrono::Timelike; // false warning
use futures::StreamExt;
use log::info;
use sqlx::SqlitePool;

use crate::{CalendarEvent, UserId};

mod graphql;
mod main_page;

crate::define_user_data! {
    #[table_name = "naver_user"]
    #[base_url = "https://m.booking.naver.com/"]
    struct NaverUser {
        #[session_name = "NID_AUT"]
        aut: String,
        #[session_name = "NID_SES"]
        ses: String,
    }
}

impl NaverUser {
    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `aut`, `ses` FROM `naver_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get naver_user"))
    }
}

#[async_trait]
impl crate::UserImpl for NaverUser {
    type Detail = NaverUserDetail;

    const PING_INTERVAL: Option<std::time::Duration> = None;

    async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool> {
        let jar = self.to_cookie_jar();

        let mut scrapped_reservations = main_page::fetch(&jar).await?;
        scrapped_reservations.extend(graphql::fetch(&jar).await?);

        if scrapped_reservations.is_empty() {
            Ok(false)
        } else {
            let updated_item_count =
                CalendarEvent::upsert_events_to_db(self.user_id, &db, scrapped_reservations.iter())
                    .await?;
            info!("updated item count: {updated_item_count}",);

            Ok(updated_item_count > 0)
        }
    }

    async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `aut`, `ses` FROM `naver_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get naver_user of {user_id:?}"))
    }

    async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()> {
        sqlx::query!(
            "UPDATE `naver_user` SET `ses` = ?, `aut` = ? WHERE `user_id` = ?",
            self.ses,
            self.aut,
            self.user_id
        )
        .execute(&db)
        .await
        .context("Failed to update naver user session data")
        .map(|_| ())
    }
}

pub fn web_router() -> Router {
    crate::user_web_router::<NaverUser>()
}
