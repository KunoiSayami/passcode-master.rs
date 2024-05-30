use clap::arg;
use config::Config;
use database::DatabaseHandle;
use log::error;
use tap::TapFallible;

mod config;
mod database;
mod platform;
mod private;
mod types;
pub mod web;
use std::io::Write;

async fn async_main(config: String) -> anyhow::Result<()> {
    let config = Config::load(&config)
        .await
        .tap_err(|e| error!("Load configure error: {:?}", e))?;

    let (database, operator, broadcast) = DatabaseHandle::connect(config.database())
        .await
        .tap_err(|e| error!("Load database error: {:?}", e))?;

    let web = tokio::spawn(web::route(config.clone(), broadcast.resubscribe()));

    let bot = platform::bot(&config)?;

    let code_master = private::CodeStaff::start(bot.clone(), operator.clone(), broadcast);

    platform::bot_run(bot, config, operator.clone().into()).await?;

    operator.terminate().await;

    code_master.wait().await?;

    database
        .wait()
        .await
        .tap_err(|e| error!("Database error: {:?}", e))?;

    web.await??;
    Ok(())
}

fn init_log(systemd: bool) {
    let mut builder = env_logger::Builder::from_default_env();
    builder
        .filter_module("hyper", log::LevelFilter::Warn)
        .filter_module("cookie_store", log::LevelFilter::Warn)
        .filter_module("rustls", log::LevelFilter::Warn);

    if systemd {
        builder.format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()));
    }
    builder.init();
}

fn main() -> anyhow::Result<()> {
    let matches = clap::command!()
        .args(&[
            arg!([CONFIG] "Configure file").default_value("config.toml"),
            arg!(--systemd "Disable time output in log"),
        ])
        .get_matches();

    init_log(matches.get_flag("systemd"));

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(
            matches.get_one::<String>("CONFIG").unwrap().to_string(),
        ))
}
