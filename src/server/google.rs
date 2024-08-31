use axum::{
    response::{IntoResponse, Response},
    routing::post,
    Extension, Form, Json,
};
use hyper::StatusCode;
use sqlx::SqlitePool;
use tracing::error;

use super::user::UserSession;
use tower_sessions::Session;

#[derive(serde::Deserialize)]
struct ConfigForm {
    calendar_id: String,
}

async fn update_config(
    session: Session,
    Extension(db): Extension<SqlitePool>,
    Form(form): Form<ConfigForm>,
) -> Response {
    let user = match session.get::<UserSession>(UserSession::SESSION_KEY).await {
        Err(e) => {
            error!("Failed to get session - {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json("Internal server error"),
            )
                .into_response();
        }
        Ok(None) => {
            return (StatusCode::UNAUTHORIZED, Json("Unauthorized")).into_response();
        }
        Ok(Some(user)) => user,
    };

    match sqlx::query!(
        "UPDATE `google_user` SET `calendar_id` = ? WHERE `user_id` = ?",
        form.calendar_id,
        user.user_id.0
    )
    .execute(&db)
    .await
    {
        Err(e) => {
            error!("Failed to save config into db - {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json("Internal server error"),
            )
                .into_response();
        },
        Ok(r) => {
            if r.rows_affected() == 0 {
                error!("Could not save config into db - User is not found. remove session");
                if let Err(e) = session.remove::<UserSession>(UserSession::SESSION_KEY).await {
                    error!("Failed to remove session - {e:?}");
                }

                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json("Internal server error"),
                )
                    .into_response();
            }
        }
    }

    (Json(true)).into_response()
}

pub fn web_router<S: Sync + Send + Clone + 'static>() -> axum::Router<S> {
    axum::Router::new().route("/config", post(update_config))
}
