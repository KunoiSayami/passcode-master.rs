use argon2::{Argon2, PasswordVerifier};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
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
    disabled: bool,
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

    pub fn login_recently(&self, limit: i64) -> bool {
        kstool::time::get_current_second() as i64 - self.last_login < limit
    }

    pub fn belong(&self) -> i64 {
        self.belong
    }

    pub fn disabled(&self) -> bool {
        self.disabled
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

#[derive(Clone, Debug, FromRow)]
pub struct HistoryRow {
    timestamp: i64,
    id: String,
    code: String,
    error: Option<String>,
}

impl HistoryRow {
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }

    pub fn time(&self) -> String {
        let time = DateTime::from_timestamp(self.timestamp(), 0).unwrap();
        time.with_timezone(&chrono_tz::Asia::Taipei)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl std::fmt::Display for HistoryRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} {} {}",
            self.time(),
            self.id(),
            self.code(),
            self.error().unwrap_or("N/A")
        )
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct MetaRow {
    key: String,
    value: String,
}

impl MetaRow {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn into_value(self) -> String {
        self.value
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VStats {
    v: String,
    last: u64,
}

impl VStats {
    pub fn v(&self) -> &str {
        &self.v
    }

    pub fn last(&self) -> u64 {
        self.last
    }
    pub fn new(v: String) -> Self {
        Self {
            v,
            last: kstool::time::get_current_second(),
        }
    }
    pub fn json(self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}
