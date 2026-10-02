//! KaiBot — Discord-бот на Serenity.
//!
//! Слои, зависимости направлены сверху вниз:
//! * [`app`] — запуск процесса: Discord API, регистрация команд, Gateway, завершение;
//! * [`commands`] — возможности бота, по модулю на команду;
//! * [`framework`] — инфраструктура взаимодействий, ничего не знающая о конкретных командах;
//! * [`config`], [`state`], [`error`] — общие типы.

mod app;
mod commands;
mod config;
mod error;
mod framework;
mod state;

use std::process::ExitCode;

use tracing::{error, warn};
use tracing_subscriber::EnvFilter;

use crate::error::Chain;

#[tokio::main]
async fn main() -> ExitCode {
    // `.env` читается до инициализации журнала, чтобы `RUST_LOG` из него тоже учитывался.
    let dotenv = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,kaibot=debug")),
        )
        .init();

    // Отсутствие `.env` — норма (переменные могут прийти из окружения), битый файл — нет.
    if let Err(e) = dotenv
        && !e.not_found()
    {
        warn!("Не удалось прочитать .env: {}", Chain(&e));
    }

    match app::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("Критическая ошибка: {}", Chain(&e));
            ExitCode::FAILURE
        }
    }
}
