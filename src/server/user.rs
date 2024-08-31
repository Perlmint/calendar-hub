use aead::{rand_core::OsRng, NewAead};
use axum::{
    extract::Query,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Extension, Form, Json,
};
use chacha20poly1305::{ChaCha20Poly1305, Key};
use dioxus_logger::tracing::error;
use google_calendar3::oauth2::ApplicationSecret;
use hyper::StatusCode;
use pwbox::{pure::PureCrypto, ErasedPwBox, Eraser, Error as PwError, Suite};
use secure_string::SecureBytes;
use sqlx::SqlitePool;
use tower_sessions::Session;

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

#[derive(serde::Deserialize, Clone)]
struct LogoutQuery {
    redirect_to: Option<String>,
}

async fn logout(session: Session, Query(query): Query<LogoutQuery>) -> Response {
    if let Err(e) = session
        .remove::<UserSession>(UserSession::SESSION_KEY)
        .await
    {
        error!("Failed to removing session - {e:?}");
    }

    let uri = if let Some(uri) = &query.redirect_to {
        uri
    } else {
        "/"
    };

    Redirect::to(uri).into_response()
}

#[derive(serde::Deserialize)]
struct UnlockQuery {
    password: String,
}

#[derive(Debug, thiserror::Error)]
enum PrepareKeyError {
    #[error("password is wrong")]
    PasswordError,
    #[error("internal server error")]
    InternalError,
}

fn prepare_key(
    encrypted_key: Option<Vec<u8>>,
    password: String,
) -> Result<(Key, Vec<u8>), PrepareKeyError> {
    let key = if let Some(encrypted_key) = encrypted_key {
        let Ok(key_box) = ciborium::from_reader(std::io::Cursor::new(encrypted_key))
            .map_err(|e| {
                error!("Failed to deserialize - {e:?}");
            })
            .and_then(|key_box: ErasedPwBox| {
                let mut eraser = Eraser::new();
                eraser.add_suite::<PureCrypto>();
                match eraser.restore(&key_box) {
                    Ok(key) => Ok(key),
                    Err(e) => {
                        error!("Failed to deserialize symmetric key - {e:?}");
                        Err(())
                    }
                }
            })
        else {
            return Err(PrepareKeyError::InternalError);
        };
        let key = match key_box.open(&password) {
            Ok(key) => Key::from_exact_iter(key.into_iter().copied()),
            Err(PwError::MacMismatch) => {
                return Err(PrepareKeyError::PasswordError); // (StatusCode::BAD_REQUEST, Json("Password error")).into_response();
            }
            Err(e) => {
                error!("Failed to decrypt symmetric key - {e:?}");
                return Err(PrepareKeyError::InternalError);
            }
        };

        let Some(key) = key else {
            error!("Failed to convert symmetric key");
            return Err(PrepareKeyError::InternalError);
        };

        key
    } else {
        let key: Key = ChaCha20Poly1305::generate_key(&mut OsRng);

        key
    };

    let Ok(key_box) = PureCrypto::build_box(&mut OsRng)
        .seal(password, &key)
        .map_err(|e| error!("Failed to encrypt new key - {e:?}"))
    else {
        return Err(PrepareKeyError::InternalError);
    };
    let mut eraser = Eraser::new();
    eraser.add_suite::<PureCrypto>();
    let Ok(key_box) = eraser
        .erase(&key_box)
        .map_err(|e| error!("Failed to prepare key serialization - {e:?}"))
    else {
        return Err(PrepareKeyError::InternalError);
    };
    let mut encrypted_key = Vec::<u8>::new();
    let Ok(_) = ciborium::into_writer(&key_box, &mut encrypted_key)
        .map_err(|e| error!("Failed to serialize encrypted key - {e:?}"))
    else {
        return Err(PrepareKeyError::InternalError);
    };

    Ok((key, encrypted_key))
}

async fn unlock_or_generate(
    session: Session,
    Extension(db): Extension<SqlitePool>,
    Form(query): Form<UnlockQuery>,
) -> Response {
    let Ok(user_session) = session
        .get::<UserSession>(UserSession::SESSION_KEY)
        .await
        .map_err(|e| error!("Failed to get session - {e:?}"))
    else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("Internal server error"),
        )
            .into_response();
    };
    let Some(mut user_session) = user_session else {
        return (StatusCode::UNAUTHORIZED, Json("Unauthorized")).into_response();
    };

    let Ok(encrypted_key) = sqlx::query!(
        "SELECT `encrypted_key` as `encrypted_key: Vec<u8>` FROM `keychain` WHERE `user_id` = ?",
        user_session.user_id
    )
    .fetch_optional(&db)
    .await
    .map_err(|e| error!("Failed to fetch encrypted key from key chain - {e:?}"))
    .map(|v| v.map(|r| r.encrypted_key)) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("Internal server error"),
        )
            .into_response();
    };

    let (key, encrypted_key) = match prepare_key(encrypted_key, query.password) {
        Ok(ret) => ret,
        Err(PrepareKeyError::InternalError) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json("Internal server error"),
            )
                .into_response();
        }
        Err(PrepareKeyError::PasswordError) => {
            user_session.key = UserKey::Locked(match user_session.key {
                UserKey::Locked(i) => i + 1,
                _ => 1,
            });

            if let Err(e) = session.insert(UserSession::SESSION_KEY, user_session).await {
                error!("Failed to update session - {e:?}");
            }

            return (StatusCode::BAD_REQUEST, Json("password error")).into_response();
        }
    };

    if let Err(e) = sqlx::query!(
        r#"INSERT INTO `keychain`
        (`user_id`, `encrypted_key`)
        VALUES
        (?, ?)
        ON CONFLICT DO UPDATE SET
        `encrypted_key`=`excluded`.`encrypted_key`"#,
        user_session.user_id,
        encrypted_key
    )
    .execute(&db)
    .await
    {
        error!("Failed to insert keychain - {e:?}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("Internal server error"),
        )
            .into_response();
    }

    user_session.key = UserKey::Unlocked(SecureBytes::new(key.into_iter().collect()));
    if let Err(e) = session.insert(UserSession::SESSION_KEY, user_session).await {
        error!("Failed to update session - {e:?}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("Internal server error"),
        )
            .into_response();
    }

    (StatusCode::OK).into_response()
}

pub fn web_router<S: Sync + Send + Clone + 'static>(
    api_secret: ApplicationSecret,
) -> axum::Router<S> {
    axum::Router::new()
        .route("/unlock", post(unlock_or_generate))
        .route("/logout", get(logout))
        .nest("/google", google::web_router(api_secret))
}
