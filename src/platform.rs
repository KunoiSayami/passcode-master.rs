use std::sync::Arc;

use anyhow::anyhow;
use log::warn;
use once_cell::sync::Lazy;
use tap::TapFallible;
use teloxide::{
    adaptors::DefaultParseMode,
    dispatching::{Dispatcher, HandlerExt, UpdateFilterExt},
    macros::BotCommands,
    payloads::{EditMessageTextSetters, SendMessageSetters},
    prelude::dptree,
    requests::{Requester, RequesterExt},
    types::{
        CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Message, MessageId,
        ParseMode, Update,
    },
    Bot,
};

use crate::{config::Config, database::DatabaseHelper, types::AccessLevel};

static PASSCODE_RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"^[\w\d]{5,}$").unwrap());

pub static TELEGRAM_ESCAPE_RE: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"([_*\[\]\(\)~`>#\+-=|\{}\.!])").unwrap());

static VALID_CODENAME: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"^(Agent_\d{5,}|[\w\d]{3,})$").unwrap());

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    Auth { code: String },
    Cookie { ops: String },
    Log { id: String },
    Resent { code: String },
    Invite,
    Ping,
}

#[derive(Clone, Debug)]
pub struct NecessaryArg {
    database: DatabaseHelper,
    admin: Vec<ChatId>,
    totp: totp_rs::TOTP,
    target: i64,
}

impl NecessaryArg {
    pub fn new(
        database: DatabaseHelper,
        admin: Vec<ChatId>,
        target: i64,
        totp: totp_rs::TOTP,
    ) -> Self {
        Self {
            database,
            admin,
            target,
            totp,
        }
    }

    pub fn database(&self) -> &DatabaseHelper {
        &self.database
    }

    pub fn admin(&self) -> &[ChatId] {
        &self.admin
    }

    pub fn target(&self) -> ChatId {
        ChatId(self.target)
    }

    pub async fn check_auth(&self, id: ChatId, level: AccessLevel) -> bool {
        self.check_admin(id)
            || level.required(
                self.database()
                    .user_query(id.0)
                    .await
                    .flatten()
                    .map(|u| u.authorized())
                    .unwrap_or(0),
            )
    }
    pub async fn access_level(&self, id: ChatId) -> Option<AccessLevel> {
        self.database
            .user_query(id.0)
            .await
            .flatten()
            .map(|u| AccessLevel::f_i32(u.authorized()))
    }

    pub fn check_admin(&self, id: ChatId) -> bool {
        self.admin.iter().any(|x| &id == x)
    }
}

#[derive(Debug)]
pub struct ReadableCallbackQuery<'a> {
    head: &'a str,
    action: &'a str,
    target: &'a str,
}

impl<'a> ReadableCallbackQuery<'a> {
    pub fn new(data: &'a str) -> Option<Self> {
        if !data.contains(' ') {
            return None;
        }
        let (head, right) = data.split_once(' ').unwrap();
        if !right.contains(' ') {
            return None;
        }
        let (action, target) = right.split_once(' ').unwrap();
        Some(Self {
            action,
            target,
            head,
        })
    }

    pub fn target_i64(&self) -> Option<i64> {
        let (n, index) =
            atoi::FromRadix10SignedChecked::from_radix_10_signed_checked(self.target.as_bytes());
        if index == 0 || index != self.target.len() {
            return None;
        }
        n
    }
}

#[derive(Debug)]
pub enum CookieOps<'a> {
    Toggle(&'a str, bool),
    Modify(&'a str, &'a str, &'a str),
    Query(Option<&'a str>),
}

impl<'a> CookieOps<'a> {
    fn try_parse(input: &'a str) -> Option<(&'a str, &'a str)> {
        let mut csrf = "";
        let mut session = "";

        for line in input.split_whitespace() {
            let line = line.trim();
            if line.contains('=') {
                let (left, right) = line.split_once("=").unwrap();

                let end = if right.ends_with(";") {
                    right.len() - 1
                } else {
                    right.len()
                };

                if left.eq("csrftoken") {
                    csrf = &right[..end];
                } else if left.eq("sessionid") {
                    session = &right[..end];
                }
                if !csrf.is_empty() && !session.is_empty() {
                    return Some((csrf, session));
                }
            }
        }
        None
    }
}

impl<'a> TryFrom<&'a str> for CookieOps<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        if !value.contains(' ') && !value.eq("query") {
            return Err(anyhow!("Missing space"));
        }
        let group = value.trim().split_whitespace().collect::<Vec<_>>();
        if !match group[0] {
            "enable" | "disable" => group.len() > 1,
            "modify" | "add" => group.len() > 3,
            "query" => true,
            _ => false,
        } {
            return Err(anyhow!("Mismatch argument count / Unknown ops"));
        }
        let arg = match group[0] {
            "enable" | "disable" => Self::Toggle(group[1], group[0].eq("enable")),
            "modify" | "add" => {
                if value.contains("=") {
                    if let Some((csrf, session)) = Self::try_parse(value) {
                        Self::Modify(group[1], csrf, session)
                    } else {
                        return Err(anyhow!("Unexpected ="));
                    }
                } else {
                    Self::Modify(group[1], group[2], group[3])
                }
            }
            "query" => Self::Query(group.get(1).copied()),
            _ => unreachable!(),
        };
        Ok(arg)
    }
}

pub fn bot(config: &Config) -> anyhow::Result<BotType> {
    let bot = Bot::new(config.platform().key());
    Ok(match config.platform().server() {
        Some(url) => bot.set_api_url(url.parse()?),
        None => bot,
    }
    .parse_mode(ParseMode::MarkdownV2))
}

pub type BotType = DefaultParseMode<Bot>;

pub async fn bot_run(
    bot: BotType,
    config: Config,
    database: DatabaseHelper,
    totp: totp_rs::TOTP,
) -> anyhow::Result<()> {
    let arg = Arc::new(NecessaryArg::new(
        database,
        config.admin().iter().map(|u| ChatId(*u)).collect(),
        config.platform().target(),
        totp,
    ));

    let handle_message = Update::filter_message()
        .branch(
            dptree::entry()
                .filter(|msg: Message| msg.chat.is_private())
                .filter_command::<Command>()
                .endpoint(
                    |msg: Message, bot: BotType, arg: Arc<NecessaryArg>, cmd: Command| async move {
                        match cmd {
                            Command::Auth { code } => {
                                handle_auth_command(bot, arg, msg, code).await
                            }
                            Command::Cookie { ops } => {
                                handle_cookie_command(bot, arg, msg, ops).await
                            }
                            Command::Log { id } => handle_log_command(bot, msg, arg, id).await,
                            Command::Ping => handle_ping(bot, msg, arg).await,
                            Command::Resent { code } => handle_resent(bot, msg, arg, code).await,
                            Command::Invite => handle_get_invite(bot, msg, arg).await,
                        }
                        .tap_err(|e| log::error!("Handle command error: {:?}", e))
                    },
                ),
        )
        .branch(
            dptree::entry()
                .filter(|msg: Message| {
                    msg.chat.is_private() && msg.text().is_some_and(|s| !s.starts_with('/'))
                })
                .endpoint(
                    |msg: Message, bot: BotType, arg: Arc<NecessaryArg>| async move {
                        handle_message(bot, msg, arg).await
                    },
                ),
        );

    let handle_callback_query = Update::filter_callback_query()
        .filter(|q: CallbackQuery| q.data.is_some())
        .endpoint(
            |q: CallbackQuery, bot: BotType, arg: Arc<NecessaryArg>| async move {
                handle_callback_query(bot, q, arg).await
            },
        );
    let dispatcher = Dispatcher::builder(
        bot,
        dptree::entry()
            .branch(handle_message)
            .branch(handle_callback_query),
    )
    .dependencies(dptree::deps![arg])
    .default_handler(|_| async {});

    #[cfg(not(debug_assertions))]
    dispatcher.enable_ctrlc_handler().build().dispatch().await;

    #[cfg(debug_assertions)]
    tokio::select! {
        _ = async move {
            dispatcher.build().dispatch().await
        } => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}

pub async fn handle_auth_command(
    bot: BotType,
    arg: Arc<NecessaryArg>,
    msg: Message,
    code: String,
) -> anyhow::Result<()> {
    if arg.check_admin(msg.chat.id)
        || arg
            .database()
            .user_query(msg.chat.id.0)
            .await
            .flatten()
            .is_some()
    {
        return Ok(());
    }

    if code.is_empty() && !arg.totp.check(&code, kstool::time::get_current_second()) {
        log::debug!(
            "Unexpected auth command from {}({})",
            msg.chat.first_name().unwrap_or("<NO Name>"),
            msg.chat.id.0
        );
        return Ok(());
    }

    for admin in arg.admin() {
        bot.send_message(
            *admin,
            format!(
                "User {}\\([{user}](tg://user?id={user})\\) request to grant talk power",
                TELEGRAM_ESCAPE_RE
                    .replace_all(msg.chat.first_name().unwrap_or("<NO NAME\\>"), "\\$1"),
                user = msg.chat.id.0
            ),
        )
        .reply_markup(mark_auth_keyboard(msg.chat.id.0))
        .await?;
    }
    arg.database().user_add(msg.chat.id.0).await;
    Ok(())
}

pub async fn handle_cookie_command(
    bot: BotType,
    arg: Arc<NecessaryArg>,
    msg: Message,
    ops: String,
) -> anyhow::Result<()> {
    if !arg.check_auth(msg.chat.id, AccessLevel::Cookie).await {
        return Ok(());
    }
    let ops = match CookieOps::try_from(ops.as_str()) {
        Ok(ops) => ops,
        Err(e) => {
            log::error!("Cookie arg: {:?}", e);
            return Ok(());
        }
    };
    match ops {
        CookieOps::Toggle(id, enabled) => {
            if !(arg.check_admin(msg.chat.id)
                || arg
                    .database()
                    .cookie_query_id(id.to_string())
                    .await
                    .flatten()
                    .is_some_and(|c| c.belong_chat().eq(&msg.chat.id)))
            {
                return Ok(());
            }
            arg.database().cookie_toggle(id.to_string(), enabled).await;

            bot.send_message(msg.chat.id, format!("Toggle {id} to {enabled}"))
                .await?;
        }
        CookieOps::Modify(id, csrf, session) => {
            //log::debug!("{id:?}");
            if !VALID_CODENAME.is_match(id) {
                bot.send_message(msg.chat.id, "Invalid codename").await?;
                return Ok(());
            }

            if !arg.check_admin(msg.chat.id)
                && !arg
                    .database()
                    .cookie_check_capacity(id.to_string(), msg.chat.id.0, 2)
                    .await
                    .unwrap_or(true)
            {
                bot.send_message(msg.chat.id, "Max cookie capacity exceed, if you want more capacity, please contact administrator").await?;
                return Ok(());
            }

            arg.database()
                .cookie_set(
                    msg.chat.id.0,
                    id.to_lowercase(),
                    csrf.to_string(),
                    session.to_string(),
                )
                .await;

            bot.send_message(msg.chat.id, format!("Updated {} cookie", id))
                .await?;
        }
        CookieOps::Query(additional) => {
            let cookies =
                if additional.is_some_and(|s| s.eq("all")) && arg.check_admin(msg.chat.id) {
                    arg.database().cookie_query_all(false).await
                } else if arg.check_auth(msg.chat.id, AccessLevel::Cookie).await {
                    arg.database().cookie_query(msg.chat.id.0).await
                } else {
                    return Ok(());
                }
                .unwrap();

            let cookies = cookies
                .into_iter()
                .map(|cookie| cookie.to_string())
                .collect::<Vec<_>>()
                .join("\n");

            bot.send_message(
                msg.chat.id,
                if cookies.is_empty() {
                    "Nothing to display".to_string()
                } else {
                    cookies
                },
            )
            .await?;
            return Ok(());
        }
    }
    Ok(())
}

pub async fn handle_log_command(
    bot: BotType,
    msg: Message,
    arg: Arc<NecessaryArg>,
    id: String,
) -> anyhow::Result<()> {
    if !arg.check_admin(msg.chat.id) {
        return Ok(());
    }

    match arg.database().log_query(id).await {
        Some(v) => {
            let text = v
                .iter()
                .map(|entry| entry.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                bot.send_message(msg.chat.id, "__Nothing to display__")
                    .await?;
                return Ok(());
            }

            bot.send_message(msg.chat.id, TELEGRAM_ESCAPE_RE.replace_all(&text, "\\$1"))
                .await?;
        }
        None => {
            bot.send_message(msg.chat.id, "__Nothing to display__")
                .await?;
        }
    }
    Ok(())
}

pub async fn handle_resent(
    bot: BotType,
    msg: Message,
    arg: Arc<NecessaryArg>,
    code: String,
) -> anyhow::Result<()> {
    if !arg.check_admin(msg.chat.id) {
        return Ok(());
    }
    arg.database().code_resent(code.clone()).await;
    bot.send_message(msg.chat.id, format!("`{code}` resent",))
        .await?;
    Ok(())
}

pub async fn handle_ping(bot: BotType, msg: Message, arg: Arc<NecessaryArg>) -> anyhow::Result<()> {
    bot.send_message(
        msg.chat.id,
        format!(
            "Chat id: `{id}`\nAccess level: {is_authorized}\nIs admin: {is_admin}\nVersion: {version}",
            id = msg.chat.id.0,
            is_authorized = arg
                .access_level(msg.chat.id)
                .await
                .map(|l| l.into())
                .unwrap_or("Not found"),
            is_admin = arg.check_admin(msg.chat.id),
            version = TELEGRAM_ESCAPE_RE.replace_all(env!("CARGO_PKG_VERSION"), "\\$1")
        ),
    )
    .await?;
    Ok(())
}

pub async fn handle_get_invite(
    bot: BotType,
    msg: Message,
    arg: Arc<NecessaryArg>,
) -> anyhow::Result<()> {
    if !arg.check_admin(msg.chat.id) {
        return Ok(());
    }

    bot.send_message(
        msg.chat.id,
        format!(
            "Use `/auth {}` to get authorized",
            arg.totp.generate_current().unwrap()
        ),
    )
    .await?;

    Ok(())
}

pub async fn handle_message(
    bot: BotType,
    msg: Message,
    arg: Arc<NecessaryArg>,
) -> anyhow::Result<()> {
    if !arg.check_auth(msg.chat.id, AccessLevel::Send).await {
        return Ok(());
    }
    for code in msg.text().unwrap().lines() {
        if !PASSCODE_RE.is_match(code) {
            warn!(
                "Ignore wrong format passcode {} sent by {}({})",
                code,
                msg.chat.first_name().unwrap_or("<NO NAME>"),
                msg.chat.id.0
            );
            continue;
        }
        if let Some(Some(c)) = arg.database().code_query(code.to_string()).await {
            if c.is_fr() {
                bot.send_message(msg.chat.id, format!("`{}` already FR", code))
                    .await?;
            } else {
                bot.send_message(msg.chat.id, format!("`{}` has been sent", code))
                    .reply_markup(make_fr_keyboard(code))
                    .await?;
            }
        } else {
            let msg = bot
                .send_message(arg.target(), format!("`{}`", code))
                .await?;
            arg.database.code_add(code.to_string(), msg.id.0).await;
        }
    }

    Ok(())
}

pub async fn handle_callback_query(
    bot: BotType,
    msg: CallbackQuery,
    arg: Arc<NecessaryArg>,
) -> anyhow::Result<()> {
    if msg.data.is_none() {
        return Ok(());
    }
    let data = msg.data.unwrap();

    let cq = ReadableCallbackQuery::new(&data);
    if let Some(cq) = cq {
        match cq.head {
            "user" => match cq.action {
                "all" | "cookie" | "message" => {
                    if let Some(id) = cq.target_i64() {
                        arg.database()
                            .user_approve(
                                id,
                                match cq.action {
                                    "all" => AccessLevel::All,
                                    "cookie" => AccessLevel::Cookie,
                                    "message" => AccessLevel::Send,
                                    _ => {
                                        log::warn!("Match default branch");
                                        AccessLevel::Cookie
                                    }
                                },
                            )
                            .await;
                        bot.send_message(ChatId(id), "Talk power granted").await?;
                        log::info!("{} grant {} power", msg.from.id.0, id);
                    }
                }
                "reject" => {
                    if let Some(id) = cq.target_i64() {
                        arg.database().user_revoke(id).await;
                    }
                }
                _ => {}
            },
            "code" => {
                if cq.action.eq("fr") {
                    if let Some(Some(code)) = arg.database().code_fr(cq.target.to_string()).await {
                        bot.edit_message_text(
                            arg.target(),
                            MessageId(code.message_id()),
                            format!("<del>{}</del>", code.code()),
                        )
                        .parse_mode(ParseMode::Html)
                        .await?;
                    }
                }
            }
            _ => {
                warn!("Unreachable data: {cq:?}")
            }
        }
    }

    if let Some(original) = &msg.message {
        bot.edit_message_reply_markup(original.chat().id, original.id())
            .await?;
    }
    bot.answer_callback_query(msg.id).await?;
    Ok(())
}

pub fn make_fr_keyboard(code: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[InlineKeyboardButton::callback(
        "Mark as FR",
        format!("code fr {code}"),
    )]])
}

pub fn mark_auth_keyboard(user: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback("Cookie", format!("user cookie {}", user)),
        InlineKeyboardButton::callback("Message", format!("user message {}", user)),
        InlineKeyboardButton::callback("All", format!("user all {}", user)),
    ]])
    .append_row([InlineKeyboardButton::callback(
        "No",
        format!("user reject {}", user),
    )])
}
