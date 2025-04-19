use crate::prelude::*;
use dioxus::prelude::{server_fn::error::NoCustomError, ServerFnError};
use google_calendar3::yup_oauth2::ApplicationSecret;
use secure_string::SecureBytes;

use super::Session;

mod google;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum UserKey {
    NotExist,
    Locked(usize),
    Unlocked(secure_string::SecureBytes),
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UserSession {
    pub user_id: UserId,
    pub key: UserKey,
    pub key_pair: Option<(SecureBytes, SecureBytes)>,
}

impl UserSession {
    pub const SESSION_KEY: &'static str = "user";
}

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

impl Session {
    pub async fn get_user(&self) -> Result<UserSession, ServerFnError> {
        match self.get::<UserSession>(UserSession::SESSION_KEY).await {
            Err(e) => {
                error!("Failed to get session - {e:?}");
                Err(ServerFnError::<NoCustomError>::ServerError(
                    "Session is broken".to_string(),
                ))
            }
            Ok(None) => Err(ServerFnError::<NoCustomError>::Args(
                "Unauthorized".to_string(),
            )),
            Ok(Some(user)) => Ok(user),
        }
    }
}

pub fn web_router<S: Sync + Send + Clone + 'static>(
    api_secret: ApplicationSecret,
) -> axum::Router<S> {
    axum::Router::new().nest("/google", google::web_router(api_secret))
}
