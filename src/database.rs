use futures_util::StreamExt as _;
use helper_generator::Helper;
use log::info;
use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};

pub mod v1 {
    pub const CREATE_STATEMENT: &str = r#"
        CREATE TABLE "codes" (
            "code"	TEXT NOT NULL UNIQUE,
            "message_id"	INTEGER NOT NULL UNIQUE,
            "fr"	INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY("code")
        );

        CREATE TABLE "meta" (
            "key"	TEXT NOT NULL,
            "value"	TEXT,
            PRIMARY KEY("key")
        );

        CREATE TABLE "users" (
            "id"	INTEGER NOT NULL,
            "authorized"	INTEGER NOT NULL,
            PRIMARY KEY("id")
        );

        INSERT INTO "meta" VALUES ('version', '1');
    "#;

    pub const VERSION: &str = "1";
}

#[derive(Debug)]
pub struct Database {
    conn: sqlx::SqliteConnection,
    init: bool,
}

#[async_trait::async_trait]
pub trait DatabaseCheckExt {
    fn conn_(&mut self) -> &mut sqlx::SqliteConnection;

    async fn check_database_table(&mut self) -> sqlx::Result<bool> {
        Ok(
            sqlx::query(r#"SELECT 1 FROM sqlite_master WHERE type='table' AND "name" = 'data'"#)
                .fetch_optional(self.conn_())
                .await?
                .is_some(),
        )
    }

    async fn check_database_version(&mut self) -> sqlx::Result<bool> {
        Ok(
            sqlx::query_as::<_, (String,)>(r#"SELECT "value" FROM "meta" WHERE "key" = 'version'"#)
                .fetch_optional(self.conn_())
                .await?
                .map(|(x,)| x.eq(current::VERSION))
                .unwrap_or(false),
        )
    }

    async fn insert_database_version(&mut self) -> sqlx::Result<()> {
        sqlx::query(r#"INSERT INTO "meta" VALUES ("version", ?)"#)
            .bind(current::VERSION)
            .execute(self.conn_())
            .await?;
        Ok(())
    }

    async fn create_db(&mut self) -> sqlx::Result<()> {
        let mut executer = sqlx::raw_sql(current::CREATE_STATEMENT).execute_many(self.conn_());
        while let Some(ret) = executer.next().await {
            ret?;
        }
        Ok(())
    }
}

impl Database {
    pub async fn connect(database: &str) -> DBResult<Self> {
        let conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .create_if_missing(true)
                .filename(database),
        )
        .await?;
        Ok(Self { conn, init: false })
    }

    pub async fn init(&mut self) -> sqlx::Result<bool> {
        self.init = true;
        if !self.check_database_table().await? {
            self.create_db().await?;
            self.insert_database_version().await?;
        }
        self.check_database_version().await
    }

    pub async fn check_auth(&mut self, user: i64) -> sqlx::Result<bool> {
        if user < 0 {
            return Ok(false);
        }
        Ok(
            sqlx::query(r#"SELECT 1 FROM "users" WHERE "id" = ? AND "authorized" = 1"#)
                .bind(user)
                .fetch_optional(&mut self.conn)
                .await?
                .is_some(),
        )
    }

    pub async fn query_code(&mut self, code: &str) -> DBResult<Option<CodeRow>> {
        sqlx::query_as(r#"SELECT * FROM "codes" WHERE "code" = ? "#)
            .bind(code)
            .fetch_optional(&mut self.conn)
            .await
    }

    pub async fn insert_code(&mut self, code: &str, message_id: i32) -> DBResult<()> {
        sqlx::query(r#"INSERT INTO "codes" VALUES (?, ?)"#)
            .bind(code)
            .bind(message_id)
            .execute(&mut self.conn)
            .await?;
        Ok(())
    }

    pub async fn set_code_fr(&mut self, code: &str, is_fr: bool) -> DBResult<()> {
        sqlx::query(r#"UPDATE "codes" SET "fr" = ? WHERE "code" = ?"#)
            .bind(is_fr)
            .bind(code)
            .execute(&mut self.conn)
            .await?;
        Ok(())
    }

    pub async fn query_user(&mut self, user: i64) -> DBResult<Option<User>> {
        sqlx::query_as(r#"SELECT * FROM "users" WHERE "id" = ?"#)
            .bind(user)
            .fetch_optional(&mut self.conn)
            .await
    }

    pub async fn insert_user(&mut self, user: i64, authorized: bool) -> DBResult<()> {
        sqlx::query(r#"INSERT INTO "users" VALUES (?, ?)"#)
            .bind(user)
            .bind(authorized)
            .execute(&mut self.conn)
            .await?;
        Ok(())
    }

    pub async fn set_authorized_status(&mut self, user: i64, authorized: bool) -> DBResult<()> {
        match self.query_user(user).await? {
            Some(cur) => {
                if cur.authorized() == authorized {
                    return Ok(());
                }
                sqlx::query(r#"UPDATE "users" SET "authorized" = ? WHERE "user" = ?"#)
                    .bind(authorized)
                    .bind(user)
                    .execute(&mut self.conn)
                    .await?;
                Ok(())
            }
            None => self.insert_user(user, authorized).await,
        }
    }

    pub async fn close(self) -> DBResult<()> {
        self.conn.close().await
    }
}

impl DatabaseCheckExt for Database {
    fn conn_(&mut self) -> &mut sqlx::SqliteConnection {
        &mut self.conn
    }
}

pub type DBCallSender<T> = tokio::sync::oneshot::Sender<T>;
//pub type DBCallback<T> = tokio::sync::oneshot::Receiver<T>;

#[derive(Debug, Helper)]
pub enum DatabaseEvent {
    UserAdd {
        user: i64,
        callback: DBCallSender<bool>,
    },
    UserApprove {
        user: i64,
        callback: DBCallSender<()>,
    },
    UserRevoke {
        user: i64,
        callback: DBCallSender<()>,
    },
    UserQuery {
        user: i64,
        callback: DBCallSender<bool>,
    },
    CodeQuery {
        code: String,
        callback: DBCallSender<Option<CodeRow>>,
    },
    CodeAdd {
        code: String,
        message_id: i32,
        callback: DBCallSender<()>,
    },
    CodeFR {
        code: String,
        callback: DBCallSender<Option<CodeRow>>,
    },
    Terminate,
}

pub struct DatabaseHandle {
    handle: tokio::task::JoinHandle<DBResult<()>>,
}

impl DatabaseHandle {
    pub async fn connect(file: &str) -> anyhow::Result<(Self, DatabaseHelper)> {
        let mut database = Database::connect(file).await?;
        database.init().await?;
        let (sender, receiver) = DatabaseHelper::new(2048);
        Ok((
            Self {
                handle: tokio::spawn(Self::run(database, receiver)),
            },
            sender,
        ))
    }

    async fn run(mut database: Database, mut receiver: DatabaseEventReceiver) -> DBResult<()> {
        while let Some(event) = receiver.recv().await {
            match event {
                DatabaseEvent::UserAdd { user, callback } => {
                    let u = database.query_user(user).await?;
                    if u.is_none() {
                        database.insert_user(user, false).await?;
                        info!("Add user {} to database", user);
                    }
                    callback.send(u.is_none()).ok();
                }
                DatabaseEvent::UserApprove { user, callback } => {
                    database.set_authorized_status(user, true).await?;
                    callback.send(()).ok();
                }
                DatabaseEvent::UserRevoke { user, callback } => {
                    database.set_authorized_status(user, false).await?;
                    callback.send(()).ok();
                }

                DatabaseEvent::CodeAdd {
                    code,
                    callback,
                    message_id,
                } => {
                    database.insert_code(&code, message_id).await?;
                    callback.send(()).ok();
                }
                DatabaseEvent::CodeFR { code, callback } => {
                    database.set_code_fr(&code, true).await?;
                    let code = database.query_code(&code).await?;
                    callback.send(code).ok();
                }
                DatabaseEvent::CodeQuery { code, callback } => {
                    callback.send(database.query_code(&code).await?).ok();
                }
                DatabaseEvent::Terminate => break,
                DatabaseEvent::UserQuery { user, callback } => {
                    callback
                        .send(
                            database
                                .query_user(user)
                                .await?
                                .map(|u| u.authorized())
                                .unwrap_or(false),
                        )
                        .ok();
                }
            }
        }
        database.close().await?;
        Ok(())
    }

    pub async fn wait(self) -> anyhow::Result<()> {
        Ok(self.handle.await??)
    }
}

#[derive(Clone, Debug)]
pub struct DatabaseOperator(DatabaseHelper);

impl DatabaseOperator {
    pub async fn user_add(&self, user: i64) -> Option<bool> {
        let (s, r) = oneshot::channel();
        self.0.user_add(user, s).await?;
        r.await.ok()
    }
    pub async fn user_approve(&self, user: i64) -> Option<()> {
        let (s, r) = oneshot::channel();
        self.0.user_approve(user, s).await?;
        r.await.ok()
    }

    pub async fn user_revoke(&self, user: i64) -> Option<()> {
        let (s, r) = oneshot::channel();
        self.0.user_revoke(user, s).await?;
        r.await.ok()
    }
    pub async fn code_query(&self, code: String) -> Option<CodeRow> {
        let (s, r) = oneshot::channel();
        self.0.code_query(code, s).await?;
        r.await.ok().flatten()
    }

    pub async fn code_insert(&self, code: String, message_id: i32) -> Option<()> {
        let (s, r) = oneshot::channel();
        self.0.code_add(code, message_id, s).await?;
        r.await.ok()
    }

    pub async fn user_query(&self, user: i64) -> Option<bool> {
        let (s, r) = oneshot::channel();
        self.0.user_query(user, s).await?;
        r.await.ok()
    }

    pub async fn code_fr(&self, code: String) -> Option<CodeRow> {
        let (s, r) = oneshot::channel();
        self.0.code_f_r(code, s).await?;
        r.await.ok().flatten()
    }
}

impl From<DatabaseHelper> for DatabaseOperator {
    fn from(value: DatabaseHelper) -> Self {
        Self(value)
    }
}

pub type DBResult<T> = sqlx::Result<T>;
use tokio::sync::oneshot;
pub use v1 as current;

use crate::types::{CodeRow, User};
