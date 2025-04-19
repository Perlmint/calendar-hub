use anyhow::Context;
use chrono::Utc;
use dioxus::prelude::*;

use crate::{pages::UnlockRequired, VaultContext};

#[derive(
    Debug,
    Clone,
    serde::Deserialize,
    serde::Serialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    enum_iterator::Sequence,
)]
pub enum VaultKey {
    Cgv,
    Bustago,
    NaverReservation,
    CatchTable,
}

#[derive(PartialEq, Clone)]
pub enum VaultItemDetail {
    Unsecured(&'static str),
    Secured(&'static str),
}

#[derive(PartialEq, Clone, Props)]
pub struct VaultItemConfigProps {
    pub vault_key: VaultKey,
    pub key_values: &'static [(&'static str, VaultItemDetail)],
    pub onsubmit: EventHandler<FormEvent>,
}

#[component]
pub fn VaultItemConfig(props: VaultItemConfigProps) -> Element {
    let vault: VaultContext = use_context();

    let has_value = vault
        .as_ref()
        .map(|v| v.contains_key(&props.vault_key))
        .unwrap_or_default();

    let fields = props.key_values.iter().map(|(description, detail)| {
        let (key, input_type) = match detail {
            VaultItemDetail::Secured(k) => (*k, "password"),
            VaultItemDetail::Unsecured(k) => (*k, "text"),
        };
        rsx! {
            div {
                class: "field",
                label {
                    class: "label",
                    r#for: key,
                    {description}
                }
                div {
                    class: "control",
                    input {
                        class: "input",
                        r#type: input_type,
                        name: key,
                    }
                }
            }
        }
    });

    rsx! {
        div {
            class: "section",
            if has_value {
                article {
                    class: "message is-primary",
                    div {
                        class: "message-body",
                        "Configuration exists"
                    }
                }
            }
            div {
                class: "box",
                form {
                    onsubmit: move |e| props.onsubmit.call(e),
                    {fields}
                    div {
                        class: "field is-grouped is-grouped-right",
                        UnlockRequired {
                            div {
                                class: "control",
                                button {
                                    class: "button is-primary",
                                    r#type: "submit",
                                    "Save"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "server")]
mod server {
    use super::VaultKey;

    impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for VaultKey {
        fn encode_by_ref(
            &self,
            buf: &mut <sqlx::Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
        ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
            let value = match self {
                VaultKey::Cgv => "cgv".into(),
                VaultKey::Bustago => "bustago".into(),
                VaultKey::NaverReservation => "naver_reservation".into(),
                VaultKey::CatchTable => "catch_table".into(),
            };

            buf.push(sqlx::sqlite::SqliteArgumentValue::Text(value));

            Ok(sqlx::encode::IsNull::No)
        }
    }

    #[derive(Debug, Clone, thiserror::Error)]
    #[error("unknown value - {0}")]
    pub struct VaultKeyDecodeError(String);

    #[cfg(feature = "server")]
    impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for VaultKey {
        fn decode(
            value: <sqlx::Sqlite as sqlx::Database>::ValueRef<'r>,
        ) -> Result<Self, sqlx::error::BoxDynError> {
            use sqlx::{Value, ValueRef};
            let value: String = value.to_owned().try_decode_unchecked()?;

            let value = match value.as_str() {
                "cgv" => VaultKey::Cgv,
                "bustago" => VaultKey::Bustago,
                "naver_reservation" => VaultKey::NaverReservation,
                "catch_table" => VaultKey::CatchTable,
                _ => return Err(Box::new(VaultKeyDecodeError(value))),
            };

            Ok(value)
        }
    }

    impl sqlx::Type<sqlx::Sqlite> for VaultKey {
        fn type_info() -> <sqlx::Sqlite as sqlx::Database>::TypeInfo {
            <[u8] as sqlx::Type<sqlx::Sqlite>>::type_info()
        }
    }
}

impl std::fmt::Display for VaultKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultKey::Cgv => f.write_str("CGV"),
            VaultKey::Bustago => f.write_str("버스타고"),
            VaultKey::NaverReservation => f.write_str("네이버 예약"),
            VaultKey::CatchTable => f.write_str("캐치테이블"),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SetVaultItemParams {
    key: VaultKey,
    data: Vec<u8>,
}

impl SetVaultItemParams {
    pub fn new<D: serde::Serialize>(key: VaultKey, data: D) -> anyhow::Result<Self> {
        Ok(Self {
            key,
            data: {
                let mut buffer = Vec::<u8>::new();
                ciborium::into_writer(&data, &mut buffer).context("Failed to serialize")?;
                buffer
            },
        })
    }
}

#[server]
pub async fn set_vault_item(params: SetVaultItemParams) -> Result<(), ServerFnError> {
    use crate::server::prelude::{common::*, crypto::*, user::*};

    let session: Session = extract().await?;
    let Extension(db): Extension<SqlitePool> = extract().await?;

    let session = session.get_user().await?;

    let UserKey::Unlocked(key) = session.key else {
        return Err(ServerFnError::<NoCustomError>::Args("Locked".to_string()));
    };

    let key = Key::from_iter(key.unsecure().iter().copied());
    let nonce = Nonce::generate(|_| OsRng.next_u32() as u8);
    let cipher = ChaCha20Poly1305::new(&key);
    let encrypted = cipher.encrypt(&nonce, params.data.as_ref()).map_err(|e| {
        ServerFnError::<NoCustomError>::ServerError(format!("Failed to encrypt data - {e:?}"))
    })?;

    let nonce = nonce.as_slice();

    sqlx::query!(
        r#"
        INSERT INTO `vault` (`user_id`, `key`, `nonce`, `data`)
        VALUES (?, ?, ?, ?)
        ON CONFLICT DO UPDATE SET
        `nonce`=`excluded`.`nonce`,
        `data`=`excluded`.`data`"#,
        session.user_id,
        params.key,
        nonce,
        encrypted,
    )
    .execute(&db)
    .await
    .map_err(|e| {
        ServerFnError::<NoCustomError>::ServerError(format!(
            "Failed to save encrypted data - {e:?}"
        ))
    })?;
    let default_last_synced = chrono::DateTime::<Utc>::UNIX_EPOCH;
    sqlx::query!(
        r#"
        INSERT INTO `source` (`user_id`, `vault_key`, `last_synced`)
        VALUES (?, ?, ?)
        ON CONFLICT DO NOTHING"#,
        session.user_id,
        params.key,
        default_last_synced
    )
    .execute(&db)
    .await
    .map_err(|e| {
        ServerFnError::<NoCustomError>::ServerError(format!("Failed to new save source - {e:?}"))
    })?;

    Ok(())
}

#[cfg(feature = "server")]
pub async fn get_vault_item<T: serde::de::DeserializeOwned>(
    db: &sqlx::SqlitePool,
    key: &secure_string::SecureBytes,
    user_id: crate::server::user::UserId,
    vault_key: &VaultKey,
) -> anyhow::Result<T> {
    use crate::server::prelude::crypto::*;
    use anyhow::Context;

    let r = sqlx::query!("SELECT `nonce` as `nonce: Vec<u8>`, `data` as `data: Vec<u8>` FROM `vault` WHERE `user_id` = ? AND `key` = ?", user_id, vault_key).fetch_one(db).await?;
    let nonce =
        Nonce::from_exact_iter(r.nonce.into_iter()).context("Failed to decode saved nonce")?;
    let cipher =
        ChaCha20Poly1305::new_from_slice(key.unsecure()).context("Failed to unsecure key")?;
    let decrypted = cipher
        .decrypt(&nonce, r.data.as_slice())
        .context("Failed to encrypt data")?;

    ciborium::from_reader(&mut std::io::Cursor::new(decrypted)).context("Failed to deserialize")
}
