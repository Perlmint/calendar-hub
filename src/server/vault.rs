use axum::{Extension, Form};
use chacha20poly1305::{ChaCha20Poly1305, Key};
use sqlx::SqlitePool;
use tower_sessions::Session;

#[derive(serde::Deserialize)]
pub struct SetQuery {
    key: String,
    data: Vec<u8>,
}

async fn set_item(session: Session, ) -> Response {
    (StatusCode::SUCCESS).into_response()
}

pub fn web_router<S: Sync + Send + Clone + 'static>(
) -> axum::Router<S> {
    axum::Router::new()
        .route("/set", post(set_item))
}
