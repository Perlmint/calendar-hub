use crate::{
    prelude::*,
    user::{User, UserContext},
};
use dioxus::prelude::*;

#[component]
pub fn UserLogin() -> Element {
    let user: UserContext = use_context();
    let nav = use_navigator();

    if user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
        nav.push(super::Route::Home);
    }

    rsx! {
        a {
            href: "/user/google/login",
            "google"
        }
    }
}

#[derive(PartialEq, Clone, Props)]
pub struct UnlockRequiredProps {
    pub children: Element,
}

#[component]
pub fn UnlockRequired(props: UnlockRequiredProps) -> Element {
    let mut user: UserContext = use_context();
    let mut password = use_signal(|| "".to_string());
    let mut error = use_signal(|| Option::<String>::None);

    let is_unlocked = {
        let user = user.as_ref();
        user.map(|u| u.is_unlocked()).unwrap_or_default()
    };

    let on_submit = {
        let password = password.clone();
        move |_| async move {
            let password = password.read().clone();
            if password.is_empty() {
                error.set(Some("password is empty".to_string()));
                return;
            }

            error.set(None);
            let ret = unlock_or_generate(KeychainParams {
                password: password,
                reset: false,
            })
            .await;

            match ret {
                Err(_) => error.set(Some("Failed to unlock with server error".to_string())),
                Ok(false) => error.set(Some("Password mismatched".to_string())),
                Ok(true) => user.restart(),
            };
        }
    };

    let error = error.read();

    rsx! {
        if is_unlocked {
            {props.children}
        } else {
            if let Some(error) = error.as_ref() {
                article {
                    class: "message is-danger",
                    div {
                        class: "message-body",
                        {error.as_str()}
                    }
                }
            }
            form {
                div {
                    class: "field has-addons",
                    div {
                        class: "control",
                        label {
                            class: "input",
                            r#for: "password",
                            "Unlock with"
                        }
                    }
                    div {
                        class: "control",
                        input {
                            class: "input",
                            r#type: "password",
                            placeholder: "password",
                            name: "password",
                            value: password,
                            oninput: move |event| password.set(event.value())
                        }
                    }
                    div {
                        class: "control",
                        button {
                            class: "button is-primary",
                            r#type: "button",
                            onclick: on_submit,
                            "Unlock"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn UserLock() -> Element {
    let mut user: UserContext = use_context();
    let nav = use_navigator();

    let mut password = use_signal(|| "".to_string());

    let unlock_mode = {
        let user = user.as_ref();
        user.map(|u| u.has_key()).unwrap_or_default()
    };

    let on_submit = move |evt: FormEvent| async move {
        let mut form = evt.values();
        let ret = unlock_or_generate(KeychainParams {
            password: unsafe {
                form.remove("password")
                    .unwrap_unchecked()
                    .0
                    .pop()
                    .unwrap_unchecked()
            },
            reset: false,
        })
        .await;

        user.restart();

        if let Err(_e) = ret {
            // TODO: show error
        } else {
            nav.go_back();
        };
    };

    rsx! {
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
                r#type: "password",
                name: "password",
                value: password,
                oninput: move |event| password.set(event.value())
            }
            button {
                r#type: "submit",
                if unlock_mode {
                    "Unlock"
                } else {
                    "Update"
                }
            }
        }
    }
}

#[server]
pub async fn get_user_info() -> Result<User, ServerFnError> {
    use crate::server::prelude::{common::*, user::*};

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

#[cfg(feature = "server")]
mod server {
    use crate::prelude::*;
    use aead::{rand_core::OsRng, NewAead as _};
    use chacha20poly1305::{ChaCha20Poly1305, Key};
    use pwbox::{pure::PureCrypto, ErasedPwBox, Eraser, Error as PwError, Suite as _};

    #[derive(Debug, thiserror::Error)]
    pub enum PrepareKeyError {
        #[error("password is wrong")]
        PasswordError,
        #[error("internal server error")]
        InternalError,
    }

    pub fn prepare_key(
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
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct KeychainParams {
    pub password: String,
    pub reset: bool,
}

#[server]
pub async fn unlock_or_generate(params: KeychainParams) -> Result<bool, ServerFnError> {
    use crate::server::prelude::{common::*, user::*};
    use secure_string::SecureBytes;
    use server::*;

    let session: Session = extract().await?;
    let Extension(db): Extension<SqlitePool> = extract().await?;

    let mut user_session = session
        .get::<UserSession>(UserSession::SESSION_KEY)
        .await
        .map_err(|e| {
            error!("Failed to get session - {e:?}");
            ServerFnError::<NoCustomError>::Args("Session does not exist".to_string())
        })?
        .ok_or_else(|| {
            error!("Not logged in");
            ServerFnError::<NoCustomError>::Args("Unauthorized".to_string())
        })?;

    let encrypted_key = sqlx::query!(
        "SELECT `encrypted_key` as `encrypted_key: Vec<u8>` FROM `keychain` WHERE `user_id` = ?",
        user_session.user_id
    )
    .fetch_optional(&db)
    .await
    .map_err(|e| {
        error!("Failed to fetch encrypted key from key chain - {e:?}");
        ServerFnError::<NoCustomError>::ServerError("Internal server error".to_string())
    })?
    .map(|v| v.encrypted_key);

    let (key, encrypted_key) = match prepare_key(encrypted_key, params.password) {
        Ok(ret) => ret,
        Err(PrepareKeyError::InternalError) => {
            return Err(ServerFnError::<NoCustomError>::ServerError(
                "Internal server error".to_string(),
            ));
        }
        Err(PrepareKeyError::PasswordError) => {
            user_session.key = UserKey::Locked(match user_session.key {
                UserKey::Locked(i) => i + 1,
                _ => 1,
            });

            if let Err(e) = session
                .insert(UserSession::SESSION_KEY, &user_session)
                .await
            {
                error!("Failed to update session - {e:?}");
            }

            return Ok(false);
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
        return Err(ServerFnError::<NoCustomError>::ServerError(
            "Internal server error".to_string(),
        ));
    }

    user_session.key = UserKey::Unlocked(SecureBytes::new(key.into_iter().collect()));
    if let Err(e) = session
        .insert(UserSession::SESSION_KEY, &user_session)
        .await
    {
        error!("Failed to update session - {e:?}");
        return Err(ServerFnError::<NoCustomError>::ServerError(
            "Internal server error".to_string(),
        ));
    }

    Ok(true)
}

#[server]
pub async fn logout() -> Result<(), ServerFnError> {
    use crate::server::prelude::user::*;

    let session: Session = extract().await?;

    if let Err(e) = session
        .remove::<UserSession>(UserSession::SESSION_KEY)
        .await
    {
        error!("Failed to removing session - {e:?}");
    }

    Ok(())
}
