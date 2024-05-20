use std::sync::Arc;

use log::warn;
use teloxide::{
    dispatching::{Dispatcher, HandlerExt, UpdateFilterExt},
    macros::BotCommands,
    payloads::SendMessageSetters,
    prelude::dptree,
    requests::{Requester, RequesterExt},
    types::{
        CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Message, MessageId,
        ParseMode, Update,
    },
    Bot,
};

use atoi::FromRadix10SignedChecked;

use crate::{config::Config, database::DatabaseHelper};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    Auth,
}

#[derive(Clone, Debug)]
pub struct NecessaryArg {
    database: DatabaseHelper,
    admin: Vec<ChatId>,
    target: i64,
}

impl NecessaryArg {
    pub fn new(database: DatabaseHelper, admin: Vec<ChatId>, target: i64) -> Self {
        Self {
            database,
            admin,
            target,
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
        let (action, target) = data.split_once(' ').unwrap();
        Some(Self {
            action,
            target,
            head,
        })
    }

    pub fn target_i64(&self) -> Option<i64> {
        let (n, index) = i64::from_radix_10_signed_checked(self.target.as_bytes());
        if index == 0 || index != self.target.len() {
            return None;
        }
        n
    }
}

pub fn bot(config: &Config) -> anyhow::Result<Bot> {
    let bot = Bot::new(config.platform().key());
    Ok(match config.platform().server() {
        Some(url) => bot.set_api_url(url.parse()?),
        None => bot,
    })
}

pub async fn bot_run(config: Config, database: DatabaseHelper) -> anyhow::Result<()> {
    let bot = bot(&config)?.parse_mode(ParseMode::MarkdownV2);

    let arg = Arc::new(NecessaryArg::new(
        database,
        config.admin().iter().map(|u| ChatId(*u)).collect(),
        config.platform().target(),
    ));

    let handle_message = Update::filter_message()
        .branch(
            dptree::entry()
                .filter(|msg: Message| msg.chat.is_private())
                .filter_command::<Command>()
                .endpoint(
                    |msg: Message, bot: Bot, arg: Arc<NecessaryArg>| async move {
                        handle_auth_command(bot, arg, msg).await
                    },
                ),
        )
        .branch(
            dptree::entry()
                .filter(|msg: Message| msg.chat.is_private() && msg.text().is_some())
                .endpoint(
                    |msg: Message, bot: Bot, arg: Arc<NecessaryArg>| async move {
                        handle_message(bot, msg, arg).await
                    },
                ),
        );

    let handle_callback_query = Update::filter_callback_query()
        .filter(|q: CallbackQuery| q.data.is_some())
        .endpoint(
            |q: CallbackQuery, bot: Bot, arg: Arc<NecessaryArg>| async move {
                handle_callback_query(bot, q, arg).await
            },
        );
    Dispatcher::builder(
        bot,
        dptree::entry()
            .branch(handle_message)
            .branch(handle_callback_query),
    )
    .dependencies(dptree::deps![arg])
    .enable_ctrlc_handler()
    .build()
    .dispatch()
    .await;
    Ok(())
}

pub async fn handle_auth_command(
    bot: Bot,
    arg: Arc<NecessaryArg>,
    msg: Message,
) -> anyhow::Result<()> {
    if arg.database().user_query(msg.chat.id.0).await.is_some() {
        return Ok(());
    }
    for admin in arg.admin() {
        bot.send_message(
            *admin,
            format!(
                "User [{user}](tg://user?id={user}) request to grant talk power",
                user = msg.chat.id.0
            ),
        )
        .reply_markup(mark_auth_keyboard(msg.chat.id.0))
        .await?;
    }
    arg.database().user_add(msg.chat.id.0).await;
    Ok(())
}

pub async fn handle_message(bot: Bot, msg: Message, arg: Arc<NecessaryArg>) -> anyhow::Result<()> {
    if !arg.admin().iter().any(|x| &msg.chat.id == x)
        && !arg
            .database()
            .user_query(msg.chat.id.0)
            .await
            .unwrap_or(false)
    {
        return Ok(());
    }
    let code = msg.text().unwrap();
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

    Ok(())
}

pub async fn handle_callback_query(
    bot: Bot,
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
                "auth" => {
                    if let Some(id) = cq.target_i64() {
                        arg.database().user_approve(id).await;
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
                            format!("~~{}~~", code.code()),
                        )
                        .await?;
                    }
                }
            }
            _ => {
                warn!("Unreachable data: {:?}", cq)
            }
        }
    }

    if let Some(original) = &msg.message {
        bot.edit_message_reply_markup(original.chat.id, original.id)
            .await?;
    }
    bot.answer_callback_query(msg.id).await?;
    Ok(())
}

pub fn make_fr_keyboard(code: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[InlineKeyboardButton::callback(
        "Mark as FR",
        format!("code fr {}", code),
    )]])
}

pub fn mark_auth_keyboard(user: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback("Yes", format!("user auth {}", user)),
        InlineKeyboardButton::callback("No", format!("user reject {}", user)),
    ]])
}
