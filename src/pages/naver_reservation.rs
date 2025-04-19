use dioxus::prelude::*;

use crate::{
    pages::vault::{VaultItemConfig, VaultItemDetail, VaultKey},
    prelude::*,
    VaultContext,
};

use super::vault;

#[derive(serde::Serialize, serde::Deserialize)]
struct NaverConfig {
    ses: String,
    aut: String,
}

#[component]
pub fn Page() -> Element {
    let mut vault: VaultContext = use_context();

    let on_submit = move |evt: FormEvent| {
        spawn(async move {
            let mut values = evt.values();
            let config = NaverConfig {
                ses: unsafe {
                    values
                        .remove("ses")
                        .unwrap_unchecked()
                        .0
                        .pop()
                        .unwrap_unchecked()
                },
                aut: unsafe {
                    values
                        .remove("aut")
                        .unwrap_unchecked()
                        .0
                        .pop()
                        .unwrap_unchecked()
                },
            };

            let params =
                vault::SetVaultItemParams::new(VaultKey::NaverReservation, config).unwrap();
            vault::set_vault_item(params).await.unwrap();
            vault.restart();
        });
    };

    rsx! {
        VaultItemConfig {
            onsubmit: on_submit,
            vault_key: VaultKey::NaverReservation,
            key_values: &[
                ("Cookie: SES", VaultItemDetail::Unsecured("ses")),
                ("Cookie: AUT", VaultItemDetail::Unsecured("aut"))
            ],
        }
    }
}

#[server]
pub async fn crawl() -> Result<usize, ServerFnError> {
    use super::vault::get_vault_item;

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
    let UserKey::Unlocked(key) = user.key else {
        return Err(ServerFnError::<NoCustomError>::Args(
            "keychain is locked".to_string(),
        ));
    };

    let Ok(config) =
        get_vault_item::<NaverConfig>(&db, &key, user.user_id, &VaultKey::NaverReservation).await
    else {
        return Err(ServerFnError::<NoCustomError>::ServerError(
            "Internal server error".to_string(),
        ));
    };

    let updated_count = server::crawl(config, user.user_id, &db)
        .await
        .map_err(|e| {
            error!("Failed to crawl NaverReservation - {e:?}");
            ServerFnError::<NoCustomError>::ServerError("Internal server error".to_string())
        })?;

    if updated_count > 0 {
        if let Err(e) =
            super::google_calendar::server::sync(user.user_id, service_account_key.clone(), &db)
                .await
        {
            error!("Failed to sync - {e:?}");
            return Err(ServerFnError::<NoCustomError>::ServerError(
                "Internal server error".to_string(),
            ));
        }
    }

    super::source::update_last_synced(user.user_id, VaultKey::NaverReservation, &db)
        .await
        .map_err(|_| {
            ServerFnError::<NoCustomError>::ServerError("Internal server error".to_string())
        })?;

    Ok(updated_count)
}

#[cfg(feature = "server")]
mod server;
