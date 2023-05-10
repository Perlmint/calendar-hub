use anyhow::Context;
use axum::{
    response::{IntoResponse, Response},
    Extension, Json, Router,
};
use axum_sessions::extractors::ReadableSession;
#[allow(unused_imports)]
use chrono::Timelike; // false warning
use futures::StreamExt;
use hyper::StatusCode;
use log::{debug, error, info};
use reqwest::cookie::Jar;
use sqlx::SqlitePool;

use crate::{url, CalendarEvent, UserId};

mod graphql;
mod main_page;

pub struct NaverUser {
    user_id: UserId,
    aut: String,
    ses: String,
}

impl NaverUser {
    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    fn to_cookie_jar(&self) -> Jar {
        let endpoint_base = url!("https://m.booking.naver.com/");
        let jar = Jar::default();
        jar.add_cookie_str(&format!("{}={}", "NID_AUT", self.aut), endpoint_base);
        jar.add_cookie_str(&format!("{}={}", "NID_SES", self.ses), endpoint_base);
        jar
    }

    pub async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `aut`, `ses` FROM `naver_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get naver_user of {user_id:?}"))
    }

    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `aut`, `ses` FROM `naver_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get naver_user"))
    }

    pub async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool> {
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

#[derive(serde::Serialize, serde::Deserialize)]
pub struct NaverUserDetail {
    ses: String,
    aut: String,
}

impl From<NaverUser> for NaverUserDetail {
    fn from(value: NaverUser) -> Self {
        NaverUserDetail {
            ses: value.ses,
            aut: value.aut,
        }
    }
}

impl From<(UserId, NaverUserDetail)> for NaverUser {
    fn from(value: (UserId, NaverUserDetail)) -> Self {
        Self {
            user_id: value.0,
            ses: value.1.ses,
            aut: value.1.aut,
        }
    }
}

async fn get_info(session: ReadableSession, Extension(db): Extension<SqlitePool>) -> Response {
    let Some(user_id) = session.get::<UserId>("user_id") else {
        debug!("Not logged in");
        return StatusCode::FORBIDDEN.into_response();
    };

    let naver_user = NaverUser::from_user_id(db, user_id).await.unwrap().unwrap();

    Json(NaverUserDetail::from(naver_user)).into_response()
}

async fn update_info(
    session: ReadableSession,
    Extension(db): Extension<SqlitePool>,
    Json(data): Json<NaverUserDetail>,
) -> Response {
    let Some(user_id) = session.get::<UserId>("user_id") else {
        debug!("Not logged in");
        return StatusCode::FORBIDDEN.into_response();
    };

    if let Err(e) = NaverUser::from((user_id, data)).update_session(db).await {
        error!("Error occurred while update naver session data - {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    } else {
        StatusCode::ACCEPTED.into_response()
    }
}

pub fn web_router() -> Router {
    Router::new()
        .route("/user", axum::routing::get(get_info))
        .route("/user", axum::routing::post(update_info))
}
