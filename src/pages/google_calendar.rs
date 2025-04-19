use crate::prelude::*;
use dioxus::prelude::*;

#[cfg(feature = "server")]
pub mod server;

#[component]
pub fn Page() -> Element {
    let config = use_server_future(get_user_config)?
        .value()
        .unwrap()
        .unwrap();
    let mut calendar_id = use_signal(|| config.calendar_id);

    let on_submit = move |evt: FormEvent| async move {
        let calendar_id = unsafe {
            evt.values()
                .remove("calendar_id")
                .unwrap_unchecked()
                .0
                .pop()
                .unwrap_unchecked()
        };

        let resp = update_config(ConfigParams { calendar_id }).await;

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
            class: "section",
            div {
                class: "box",
                "Give write access on your calendar"
            }
            div {
                class: "box",
                form {
                    onsubmit: on_submit,
                    div {
                        class: "field",
                        label {
                            class: "field",
                            r#for: "app_id",
                            "App ID"
                        }
                        div {
                            class: "control",
                            input {
                                class: "input",
                                r#type: "text",
                                name: "app_id",
                                value: config.app_id
                            }
                        }
                    }
                    div {
                        class: "field",
                        label {
                            class: "field",
                            r#for: "calendar_id",
                            "Calendar ID"
                        }
                        div {
                            class: "control",
                            input {
                                class: "input",
                                r#type: "text",
                                name: "calendar_id",
                                value: calendar_id,
                                oninput: move |event| calendar_id.set(event.value())
                            }
                        }
                    }
                    div {
                        class: "control",
                        button {
                            class: "button is-primary",
                            r#type: "submit",
                            "Update"
                        }
                    }
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
    use crate::server::prelude::{common::*, user::*};
    use google_calendar3::yup_oauth2::ServiceAccountKey;
    use std::sync::Arc;

    let Extension(service_account): Extension<Arc<ServiceAccountKey>> = extract().await?;
    let Extension(db): Extension<SqlitePool> = extract().await?;
    let session: Session = extract().await?;

    let app_id = service_account.client_email.clone();
    let user_id = session.get_user().await?.user_id;

    let calendar_id = sqlx::query!(
        "SELECT `calendar_id` FROM `google_user` WHERE `user_id` = ?",
        user_id
    )
    .fetch_optional(&db)
    .await
    .map_err(sqlx_error_to_dioxus_error)?
    .map(|r| r.calendar_id)
    .unwrap_or_default();

    Ok(GoogleConfig {
        app_id,
        calendar_id,
    })
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct ConfigParams {
    calendar_id: String,
}

#[server]
async fn update_config(params: ConfigParams) -> Result<(), ServerFnError> {
    use crate::server::prelude::{common::*, user::*};

    let Extension(db): Extension<SqlitePool> = extract().await?;
    let session: Session = extract().await?;
    let user = session.get_user().await?;

    let r = sqlx::query!(
        "UPDATE `google_user` SET `calendar_id` = ? WHERE `user_id` = ?",
        params.calendar_id,
        user.user_id.0
    )
    .execute(&db)
    .await
    .map_err(sqlx_error_to_dioxus_error)?;

    if r.rows_affected() == 0 {
        error!("Could not save config into db - User is not found. remove session");
        if let Err(e) = session
            .remove::<UserSession>(UserSession::SESSION_KEY)
            .await
        {
            error!("Failed to remove session - {e:?}");
        }

        Err(ServerFnError::<NoCustomError>::ServerError(
            "Internal server error".to_string(),
        ))
    } else {
        Ok(())
    }
}

#[server]
pub async fn sync() -> Result<(), ServerFnError> {
    use crate::{
        prelude::*,
        server::prelude::{common::*, user::*},
    };
    use google_calendar3::yup_oauth2::ServiceAccountKey;

    let session: Session = extract().await?;
    let user = session.get_user().await?;
    let Extension(db): Extension<SqlitePool> = extract().await?;
    let Extension(service_account_key): Extension<std::sync::Arc<ServiceAccountKey>> =
        extract().await?;

    if let Err(e) =
        super::google_calendar::server::sync(user.user_id, service_account_key.clone(), &db).await
    {
        error!("Failed to sync - {e:?}");
        return Err(ServerFnError::<NoCustomError>::ServerError(
            "Internal server error".to_string(),
        ));
    }

    Ok(())
}
