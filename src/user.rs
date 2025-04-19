use dioxus::hooks::Resource;

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub enum PublicUserKey {
    NotExist,
    Locked(usize),
    Unlocked,
}

#[cfg(feature = "server")]
impl From<&crate::server::user::UserKey> for PublicUserKey {
    fn from(value: &crate::server::user::UserKey) -> Self {
        match value {
            crate::server::user::UserKey::NotExist => crate::user::PublicUserKey::NotExist,
            crate::server::user::UserKey::Locked(retry) => {
                crate::user::PublicUserKey::Locked(*retry)
            }
            crate::server::user::UserKey::Unlocked(_) => crate::user::PublicUserKey::Unlocked,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum User {
    SignedIn(PublicUserKey),
    SignedOut,
}

impl Default for User {
    fn default() -> Self {
        User::SignedOut
    }
}

impl User {
    pub fn is_signed_in(&self) -> bool {
        matches!(self, User::SignedIn(_))
    }

    #[allow(dead_code)]
    pub fn is_locked(&self) -> bool {
        matches!(self, User::SignedIn(PublicUserKey::Locked(_)))
    }

    pub fn is_unlocked(&self) -> bool {
        matches!(self, User::SignedIn(PublicUserKey::Unlocked))
    }

    pub fn has_key(&self) -> bool {
        !matches!(self, User::SignedIn(PublicUserKey::NotExist))
    }
}

pub type UserContext = Resource<User>;
