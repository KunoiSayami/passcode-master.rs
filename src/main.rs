use clap::arg;
use config::Config;
use database::DatabaseHandle;
use log::error;
use platform::bot_run;
use tap::TapFallible;

mod config;
mod database;
mod platform;
mod types;
pub mod web;

async fn async_main(config: String) -> anyhow::Result<()> {
    let config = Config::load(&config)
        .await
        .tap_err(|e| error!("Load configure error: {:?}", e))?;

    let (database, operator) = DatabaseHandle::connect(config.database())
        .await
        .tap_err(|e| error!("Load database error: {:?}", e))?;

    bot_run(config, operator.clone().into()).await?;

    operator.terminate().await;

    database
        .wait()
        .await
        .tap_err(|e| error!("Database error: {:?}", e))?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let matches = clap::command!()
        .args(&[arg!([CONFIG] "Configure file").default_value("config.toml")])
        .get_matches();

    env_logger::Builder::from_default_env().init();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(
            matches.get_one::<String>("CONFIG").unwrap().to_string(),
        ))
}
