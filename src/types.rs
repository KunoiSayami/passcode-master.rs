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
