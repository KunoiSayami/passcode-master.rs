use argon2::{Argon2, PasswordVerifier};
use serde::Deserialize;
use sqlx::prelude::FromRow;

#[derive(Clone, Copy, Debug, FromRow)]
pub struct User {
    id: i64,
    authorized: i64,
}

impl User {
    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn authorized(&self) -> bool {
        self.authorized == 1
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct CodeRow {
    code: String,
    fr: i64,
    message_id: i32,
}

impl CodeRow {
    pub fn is_fr(&self) -> bool {
        self.fr == 1
    }

    pub fn message_id(&self) -> i32 {
        self.message_id
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct Cookie {
    id: String,
    csrf_token: String,
    session_id: String,
    last_login: i64,
    belong: i64,
}

impl Cookie {
    pub fn csrf_token(&self) -> &str {
        &self.csrf_token
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn last_login(&self) -> i64 {
        self.last_login
    }
    pub fn belong(&self) -> i64 {
        self.belong
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Auth {
    hash: String,
    codename: String,
}

impl Auth {
    pub fn codename(&self) -> &str {
        &self.codename
    }

    pub fn check(&self, origin: &str) -> bool {
        let origin_hash = match argon2::PasswordHash::new(origin) {
            Ok(hash) => hash,
            Err(e) => {
                log::error!("Original password parse error: {:?}", e);
                return false;
            }
        };
        Argon2::default()
            .verify_password(self.hash.as_bytes(), &origin_hash)
            .is_ok()
    }
}

impl TryFrom<&str> for Auth {
    type Error = serde_json::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        serde_json::from_str(value)
    }
}
