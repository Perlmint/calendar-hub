
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
