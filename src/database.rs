use futures_util::StreamExt as _;
use log::{error, info};
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

        CREATE TABLE "cookies" (
            "id"    TEXT NOT NULL,
            "csrf_token" TEXT NOT NULL,
            "session_id" TEXT NOT NULL,
            "last_login" INTEGER NOT NULL,
            "belong" INTEGER NOT NULL,
            "enabled" INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY("id")
        );

        CREATE TABLE "history" (
            "timestamp" INTEGER NOT NULL,
            "id"        TEXT NOT NULL,
            "code"      TEXT NOT NULL,
            "error"     TEXT,
            PRIMARY KEY("timestamp")
        );
    "#;

    pub const VERSION: &str = "1";

    #[derive(Clone)]
    pub enum BroadcastEvent {
        NewCode(String),
        Exit,
    }

    impl BroadcastEvent {
        pub fn new_code(code: &str) -> Self {
            Self::NewCode(code.to_string())
        }

        pub fn exit() -> Self {
            Self::Exit
        }
    }
}

#[derive(Debug)]
pub struct Database {
    conn: sqlx::SqliteConnection,
    broadcast: broadcast::Sender<current::BroadcastEvent>,
    init: bool,
}

#[async_trait::async_trait]
pub trait DatabaseCheckExt {
    fn conn_(&mut self) -> &mut sqlx::SqliteConnection;

    async fn check_database_table(&mut self) -> sqlx::Result<bool> {
        Ok(
            sqlx::query(r#"SELECT 1 FROM sqlite_master WHERE type='table' AND "name" = 'meta'"#)
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
    pub async fn connect(
        database: &str,
        broadcast: broadcast::Sender<current::BroadcastEvent>,
    ) -> DBResult<Self> {
        let conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .create_if_missing(true)
                .filename(database),
        )
        .await?;
        Ok(Self {
            conn,
            init: false,
            broadcast,
        })
    }

    pub async fn init(&mut self) -> sqlx::Result<bool> {
        self.init = true;
        if !self.check_database_table().await? {
            self.create_db().await?;
            self.insert_database_version().await?;
        }
        self.check_database_version().await
    }

    pub async fn _check_auth(&mut self, user: i64) -> sqlx::Result<bool> {
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
        sqlx::query(r#"INSERT INTO "codes" VALUES (?, ?, 0)"#)
            .bind(code)
            .bind(message_id)
            .execute(&mut self.conn)
            .await?;
        self.broadcast
            .send(current::BroadcastEvent::new_code(code))
            .ok()
            .tap_none(|| error!("Unable send broadcast"));
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
        match self.query_user(user).await.tap(|u| log::debug!("{u:?}"))? {
            Some(cur) => {
                if cur.authorized() == authorized {
                    return Ok(());
                }
                sqlx::query(r#"UPDATE "users" SET "authorized" = ? WHERE "id" = ?"#)
                    .bind(authorized)
                    .bind(user)
                    .execute(&mut self.conn)
                    .await?;
                Ok(())
            }
            None => self.insert_user(user, authorized).await,
        }
    }

    pub async fn cookie_set(
        &mut self,
        user: i64,
        csrf: &str,
        session: &str,
        id: &str,
    ) -> DBResult<bool> {
        match self.cookie_query(id).await? {
            Some(cookie) => {
                if cookie.belong() != user {
                    return Ok(false);
                }
                sqlx::query(
                    r#"UPDATE "cookies" SET "csrf_token"= ?, "session_id" = ? WHERE "id" = ?"#,
                )
                .bind(csrf)
                .bind(session)
                .bind(id)
                .execute(&mut self.conn)
                .await?;
            }
            None => {
                sqlx::query(r#"INSERT INTO "cookies" VALUES (?, ?, ?, 0, ?, 1)"#)
                    .bind(id)
                    .bind(csrf)
                    .bind(session)
                    .bind(user)
                    .execute(&mut self.conn)
                    .await?;
            }
        }
        Ok(true)
    }

    pub async fn cookie_usable(&mut self, id: &str, usable: bool) -> DBResult<()> {
        sqlx::query(r#"UPDATE "cookies" SET "enabled" = ? WHERE "id" = ?"#)
            .bind(usable)
            .bind(id)
            .execute(&mut self.conn)
            .await?;
        Ok(())
    }

    pub async fn cookie_update_timestamp(&mut self, id: &str) -> DBResult<()> {
        sqlx::query(r#"UPDATE "cookies" SET "last_login" = ? WHERE "id" = ?"#)
            .bind(kstool::time::get_current_second() as i64)
            .bind(id)
            .execute(&mut self.conn)
            .await?;
        Ok(())
    }

    pub async fn cookie_query(&mut self, id: &str) -> DBResult<Option<Cookie>> {
        sqlx::query_as(r#"SELECT * FROM "cookies" WHERE "id" = ?"#)
            .bind(id)
            .fetch_optional(&mut self.conn)
            .await
    }

    pub async fn cookie_query_user(&mut self, id: i64) -> DBResult<Vec<Cookie>> {
        sqlx::query_as(r#"SELECT * FROM "cookies" WHERE "belong" = ?"#)
            .bind(id)
            .fetch_all(&mut self.conn)
            .await
    }

    pub async fn cookie_query_all(&mut self) -> DBResult<Vec<Cookie>> {
        sqlx::query_as(r#"SELECT* FROM "cookies"  WHERE "enabled" = 1"#)
            .fetch_all(&mut self.conn)
            .await
    }

    pub async fn v_query(&mut self) -> DBResult<Option<VStats>> {
        Ok(
            sqlx::query_as::<_, MetaRow>(r#"SELECT * FROM "meta" WHERE "key" = 'intel_v'"#)
                .fetch_optional(&mut self.conn)
                .await?
                .and_then(|s| serde_json::from_str(s.value()).ok()),
        )
    }

    pub async fn v_update(&mut self, v: String) -> DBResult<()> {
        if let Some(db_v) = self.v_query().await? {
            if v.eq(db_v.v()) {
                return Ok(());
            }
            sqlx::query(r#"UPDATE "meta" SET "value" = ? WHERE "key" = 'intel_v'"#)
                .bind(VStats::new(v).json())
                .execute(&mut self.conn)
                .await
        } else {
            sqlx::query(r#"INSERT INTO "meta" VALUES ('intel_v', ?)"#)
                .bind(VStats::new(v).json())
                .execute(&mut self.conn)
                .await
        }?;
        Ok(())
    }

    pub async fn log_add(&mut self, id: &str, code: &str, error: Option<String>) -> DBResult<()> {
        sqlx::query(r#"INSERT INTO "history" VALUES (?, ?, ?, ?)"#)
            .bind(kstool::time::get_current_second() as i64)
            .bind(id)
            .bind(code)
            .bind(error)
            .execute(&mut self.conn)
            .await?;
        Ok(())
    }

    pub async fn log_query(&mut self, id: &str) -> DBResult<Vec<HistoryRow>> {
        sqlx::query_as(
            r#"SELECT * FROM "history" WHERE "id" = ? ORDER BY "timestamp" DESC LIMIT 20"#,
        )
        .bind(id)
        .fetch_all(&mut self.conn)
        .await
    }

    pub async fn log_query_all(&mut self) -> DBResult<Vec<HistoryRow>> {
        sqlx::query_as(r#"SELECT * FROM "history" ORDER BY "timestamp" DESC LIMIT 40"#)
            .fetch_all(&mut self.conn)
            .await
    }

    pub async fn close(self) -> DBResult<()> {
        self.broadcast.send(current::BroadcastEvent::exit()).ok();
        self.conn.close().await
    }
}

impl DatabaseCheckExt for Database {
    fn conn_(&mut self) -> &mut sqlx::SqliteConnection {
        &mut self.conn
    }
}

//pub type DBCallSender<T> = tokio::sync::oneshot::Sender<T>;
//pub type DBCallback<T> = tokio::sync::oneshot::Receiver<T>;

kstool_helper_generator::oneshot_helper! {
#[derive(Debug)]
pub enum DatabaseEvent {
    #[ret(bool)]
    UserAdd {
        user: i64
    },
    #[ret(())]
    UserApprove {
        user: i64,
    },
    #[ret(())]
    UserRevoke {
        user: i64,
    },
    #[ret(Option<User>)]
    UserQuery {
        user: i64,
    },
    #[ret(Option<CodeRow>)]
    CodeQuery {
        code: String,
    },
    #[ret(())]
    CodeAdd {
        code: String,
        message_id: i32,
    },
    #[ret(())]
    CodeResent {
        code: String,
    },
    #[ret(Option<CodeRow>)]
    CodeFR {
        code: String
    },

    #[ret(Vec<Cookie>)]
    CookieQueryAll,

    #[ret(Vec<Cookie>)]
    CookieQuery(i64),

    #[ret(())]
    CookieToggle {id: String, usable: bool},

    #[ret(bool)]
    CookieSet {user: i64, id: String, csrf: String, session: String},

    #[ret(())]
    CookieUpdateTimestamp(String),

    #[ret(())]
    VUpdate {v: String},

    LogInsert {
        id: String,
        code: String,
        error: Option<String>
    },

    #[ret(Vec<HistoryRow>)]
    LogQuery {id: String,},

    #[ret(Option<VStats>)]
    VQuery,

    Terminate,
}
}

pub struct DatabaseHandle {
    handle: tokio::task::JoinHandle<DBResult<()>>,
}

impl DatabaseHandle {
    pub async fn connect(
        file: &str,
    ) -> anyhow::Result<(
        Self,
        DatabaseHelper,
        broadcast::Receiver<current::BroadcastEvent>,
    )> {
        let (s, r) = broadcast::channel(32);
        let mut database = Database::connect(file, s).await?;
        database.init().await?;
        let (sender, receiver) = DatabaseHelper::new(2048);
        Ok((
            Self {
                handle: tokio::spawn(Self::run(database, receiver)),
            },
            sender,
            r,
        ))
    }

    async fn handle_event(database: &mut Database, event: DatabaseEvent) -> DBResult<()> {
        match event {
            DatabaseEvent::UserAdd {
                user,
                __private_sender,
            } => {
                let u = database.query_user(user).await?;
                if u.is_none() {
                    database.insert_user(user, false).await?;
                    info!("Add user {} to database", user);
                }
                __private_sender.send(u.is_none()).ok();
            }
            DatabaseEvent::UserApprove {
                user,
                __private_sender,
            } => {
                database.set_authorized_status(user, true).await?;
                info!("Approve user {}", user);
                __private_sender.send(()).ok();
            }
            DatabaseEvent::UserRevoke {
                user,
                __private_sender,
            } => {
                database.set_authorized_status(user, false).await?;
                __private_sender.send(()).ok();
            }

            DatabaseEvent::CodeAdd {
                code,

                message_id,
                __private_sender,
            } => {
                database.insert_code(&code, message_id).await?;
                __private_sender.send(()).ok();
            }
            DatabaseEvent::CodeFR {
                code,
                __private_sender,
            } => {
                database.set_code_fr(&code, true).await?;
                let code = database.query_code(&code).await?;
                __private_sender.send(code).ok();
            }
            DatabaseEvent::CodeQuery {
                code,
                __private_sender,
            } => {
                __private_sender
                    .send(database.query_code(&code).await?)
                    .ok();
            }
            DatabaseEvent::Terminate => unreachable!(),
            DatabaseEvent::UserQuery {
                user,
                __private_sender,
            } => {
                __private_sender.send(database.query_user(user).await?).ok();
            }
            DatabaseEvent::CookieQueryAll(sender) => {
                sender.send(database.cookie_query_all().await?).ok();
            }
            DatabaseEvent::VUpdate {
                v,
                __private_sender,
            } => {
                __private_sender.send(database.v_update(v).await?).ok();
            }

            DatabaseEvent::VQuery(sender) => {
                sender.send(database.v_query().await?).ok();
            }
            DatabaseEvent::CookieToggle {
                id,
                usable,
                __private_sender,
            } => {
                __private_sender
                    .send(database.cookie_usable(&id, usable).await?)
                    .ok();
            }
            DatabaseEvent::CookieSet {
                user,
                id,
                csrf,
                session,
                __private_sender,
            } => {
                __private_sender
                    .send(database.cookie_set(user, &csrf, &session, &id).await?)
                    .ok();
            }
            DatabaseEvent::CookieUpdateTimestamp(id, sender) => {
                sender
                    .send(database.cookie_update_timestamp(&id).await?)
                    .ok();
            }
            DatabaseEvent::LogInsert { id, code, error } => {
                database.log_add(&id, &code, error).await?;
            }
            DatabaseEvent::LogQuery {
                id,
                __private_sender,
            } => {
                __private_sender
                    .send(if id.is_empty() {
                        database.log_query_all().await
                    } else {
                        database.log_query(&id).await
                    }?)
                    .ok();
            }
            DatabaseEvent::CookieQuery(id, sender) => {
                sender.send(database.cookie_query_user(id).await?).ok();
            }
            DatabaseEvent::CodeResent {
                code,
                __private_sender,
            } => {
                database.broadcast.send(BroadcastEvent::NewCode(code)).ok();
                __private_sender.send(()).ok();
            }
        }
        Ok(())
    }

    async fn run(mut database: Database, mut receiver: DatabaseEventReceiver) -> DBResult<()> {
        while let Some(event) = receiver.recv().await {
            if let DatabaseEvent::Terminate = event {
                break;
            }
            Self::handle_event(&mut database, event)
                .await
                .tap_err(|e| error!("Sqlite error: {:?}", e))?;
        }
        database.close().await?;
        Ok(())
    }

    pub async fn wait(self) -> anyhow::Result<()> {
        Ok(self.handle.await??)
    }
}

pub type DBResult<T> = sqlx::Result<T>;
use tap::{Tap, TapFallible, TapOptional};
use tokio::sync::broadcast;
pub use v1 as current;

use crate::types::{CodeRow, Cookie, HistoryRow, MetaRow, User, VStats};

pub use current::BroadcastEvent;
