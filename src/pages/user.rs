use std::collections::HashMap;

use crate::{user::{User, UserContext}, BaseUrl};
use dioxus::prelude::*;
use server_fn::error::NoCustomError;
use tracing::info;

#[component]
pub fn UserLogin() -> Element {
    rsx! {
        a {
            href: "/user/google/login",
            "google"
        }
    }
}

#[component]
pub fn UserPasswordInput() -> Element {
    let user: UserContext = use_context();
    let unlock_mode = user.as_ref().map(|u| u.has_key()).unwrap_or_default();
    let base_url: Resource<BaseUrl> = use_context();
    let nav = use_navigator();

    let mut password = use_signal(|| "".to_string());

    let on_submit = move |evt: FormEvent| async move {
        info!("submit");
        let form: HashMap<_, _> = evt
            .values()
            .into_iter()
            .filter_map(|(key, v)| {
                (key == "password").then(|| (key, v.0.into_iter().next().unwrap_or_default()))
            })
            .collect();
        let resp = reqwest::Client::new()
            .post(format!("{}/user/keychain", base_url.value().unwrap()))
            .form(&form)
            .send()
            .await;

        match resp {
            // Parse data from here, such as storing a response token
            Ok(_data) => {
                info!("Update successful!");
                nav.go_back();
            },

            //Handle any errors from the fetch here
            Err(err) => {
                info!("Update failed - {err:?}")
            }
        }
    };

    rsx!{
        h1 {
            if unlock_mode {
                "Unlock by password"
            } else {
                "Create new key with password"
            }
        }
        form {
            onsubmit: on_submit,
            label {
                r#for: "password",
                "Password"
            }
            input {
                r#type: "text",
                name: "password",
                value: password,
                oninput: move |event| password.set(event.value())
            }
            button {
                r#type: "submit",
                "Update"
            }
        }
    }
}

#[server]
pub async fn UserInfo() -> Result<User, ServerFnError> {
    use crate::{server::user::UserSession, Session};

    let session: Session = extract().await?;
    if let Some(session) = session
        .get::<UserSession>(UserSession::SESSION_KEY)
        .await
        .map_err(|e| {
            ServerFnError::<NoCustomError>::ServerError(format!(
                "Failed to data from session - {e:?}"
            ))
        })?
    {
        Ok(User::SignedIn(From::from(&session.key)))
    } else {
        Ok(User::SignedOut)
    }
}
