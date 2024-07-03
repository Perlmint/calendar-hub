use axum::{
    async_trait,
    response::{IntoResponse as _, Response},
    Extension, Json, Router,
};
use axum_sessions::extractors::ReadableSession;
use hyper::StatusCode;
use log::{debug, error};
use sqlx::SqlitePool;

#[repr(transparent)]
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    sqlx::Type,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
#[sqlx(transparent)]
pub struct UserId(pub u32);

impl From<i64> for UserId {
    fn from(value: i64) -> Self {
        Self(value as _)
    }
}

#[macro_export]
macro_rules! define_user_data {
    (
        #[table_name = $table_name:literal]
        #[base_url = $url:literal]
        struct $name:ident {
            $(
                #[session_name = $session_name:literal]
                $session_field_name:ident: $session_field_type:ty,
            )+
            $(
                $field_name:ident: $field_type:ty,
            )*
        }
    ) => {
        paste::paste!{
            #[derive(serde::Serialize, serde::Deserialize, Default)]
            pub struct [<$name Detail>] {
                $($session_field_name: $session_field_type,)+
                $($field_name: $field_type,)*
            }

            pub struct $name {
                user_id: $crate::UserId,
                $($session_field_name: $session_field_type,)+
                $($field_name: $field_type,)*
            }

            impl $name {
                pub fn user_id(&self) -> $crate::UserId {
                    self.user_id
                }
            }

            impl From<$name> for [<$name Detail>] {
                fn from(value: $name) -> Self {
                    Self {
                        $($session_field_name: value.$session_field_name,)+
                        $($field_name: value.$field_name,)*
                    }
                }
            }

            impl From<($crate::UserId, [<$name Detail>])> for $name {
                fn from(value: ($crate::UserId, [<$name Detail>])) -> Self {
                    Self {
                        user_id: value.0,
                        $($session_field_name: value.1.$session_field_name,)+
                        $($field_name: value.1.$field_name,)*
                    }
                }
            }

            impl $name {
                fn to_cookie_jar(&self) -> reqwest::cookie::Jar {
                    let endpoint_base = crate::url!($url);
                    let jar = reqwest::cookie::Jar::default();
                    $(
                        jar.add_cookie_str(&format!("{}={}", $session_name, self.$session_field_name), endpoint_base);
                    )+
                    jar
                }

                // async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()> {
                //     sqlx::query!(
                //         concat!("UPDATE `", $table_name, "` SET `ses` = ?, `aut` = ? WHERE `user_id` = ?"),
                //         self.ses,
                //         self.aut,
                //         self.user_id
                //     )
                //     .execute(&db)
                //     .await
                //     .context("Failed to update naver user session data")
                //     .map(|_| ())
                // }
            }
        }
    };
}

#[async_trait]
pub trait UserImpl: Sized + From<(UserId, Self::Detail)> + Send + Sync + 'static {
    type Detail: serde::Serialize
        + serde::de::DeserializeOwned
        + Default
        + From<Self>
        + Sized
        + Send
        + Sync
        + 'static;
    const PING_INTERVAL: Option<std::time::Duration>;

    async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool>;
    async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>>;
    async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()>;
    async fn ping(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn get_info<U: UserImpl>(
    session: ReadableSession,
    Extension(db): Extension<SqlitePool>,
) -> Response {
    let Some(user_id) = session.get::<UserId>("user_id") else {
        debug!("Not logged in");
        return StatusCode::FORBIDDEN.into_response();
    };

    let naver_user = U::from_user_id(db, user_id).await.unwrap();

    Json(naver_user.map(U::Detail::from).unwrap_or_default()).into_response()
}

async fn update_info<U: UserImpl>(
    session: ReadableSession,
    Extension(db): Extension<SqlitePool>,
    Json(data): Json<U::Detail>,
) -> Response {
    let Some(user_id) = session.get::<UserId>("user_id") else {
        debug!("Not logged in");
        return StatusCode::FORBIDDEN.into_response();
    };

    if let Err(e) = U::from((user_id, data)).update_session(db).await {
        error!("Error occurred while update naver session data - {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    } else {
        StatusCode::ACCEPTED.into_response()
    }
}

pub fn user_web_router<U: UserImpl>() -> Router {
    Router::new()
        .route("/user", axum::routing::get(get_info::<U>))
        .route("/user", axum::routing::post(update_info::<U>))
}
