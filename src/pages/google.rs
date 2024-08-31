use std::collections::HashMap;

use dioxus::prelude::*;
use tracing::info;

use crate::BaseUrl;

#[component]
pub fn Page() -> Element {
    let config = use_server_future(get_user_config)?
        .value()
        .unwrap()
        .unwrap();
    let mut calendar_id = use_signal(|| config.calendar_id);
    let base_url: Resource<BaseUrl> = use_context();

    let on_submit = move |evt: FormEvent| async move {
        info!("submit");
        let form: HashMap<_, _> = evt
            .values()
            .into_iter()
            .filter_map(|(key, v)| {
                (key == "calendar_id").then(|| (key, v.0.into_iter().next().unwrap_or_default()))
            })
            .collect();
        let resp = reqwest::Client::new()
            .post(format!("{}/google/config", base_url.value().unwrap()))
            .form(&form)
            .send()
            .await;

        match resp {
            // Parse data from here, such as storing a response token
            Ok(_data) => info!("Update successful!"),

            //Handle any errors from the fetch here
            Err(err) => {
                info!("Update failed - {err:?}")
            }
        }
    };

    rsx! {
        div {
            form {
                onsubmit: on_submit,
                label {
                    r#for: "app_id",
                    "App ID - give write access on your calendar"
                }
                input {
                    r#type: "text",
                    name: "app_id",
                    value: config.app_id
                }
                label {
                    r#for: "calendar_id",
                    "Calendar ID"
                }
                input {
                    r#type: "text",
                    name: "calendar_id",
                    value: calendar_id,
                    oninput: move |event| calendar_id.set(event.value())
                }
                button {
                    r#type: "submit",
                    "Update"
                }
            }
        }
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct GoogleConfig {
    app_id: String,
    calendar_id: String,
}

#[server]
async fn get_user_config() -> Result<GoogleConfig, ServerFnError> {
    use axum::Extension;
    use google_calendar3::oauth2::ServiceAccountKey;
    use server_fn::error::NoCustomError;
    use sqlx::SqlitePool;
    use std::sync::Arc;

    use crate::{server::user::UserSession, Session};

    let Extension(service_account): Extension<Arc<ServiceAccountKey>> = extract().await?;
    let Extension(db): Extension<SqlitePool> = extract().await?;
    let session: Session = extract().await?;

    let app_id = service_account.client_email.clone();
    let user_id = session
        .get::<UserSession>(UserSession::SESSION_KEY)
        .await?
        .ok_or_else(|| {
            ServerFnError::<NoCustomError>::ServerError("Session is broken".to_string())
        })?
        .user_id;

    let calendar_id = sqlx::query!(
        "SELECT `calendar_id` FROM `google_user` WHERE `user_id` = ?",
        user_id
    )
    .fetch_optional(&db)
    .await
    .map_err(|e| ServerFnError::<NoCustomError>::ServerError(format!("DB error - {e:?}")))?
    .map(|r| r.calendar_id)
    .unwrap_or_default();

    Ok(GoogleConfig {
        app_id,
        calendar_id,
    })
}
