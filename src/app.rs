//! Запуск бота: от переменных окружения до работающего соединения с Gateway.

use std::collections::BTreeSet;

use serenity::all::{
    ClientBuilder, Command, CurrentApplicationInfo, GatewayIntents, Http, MembershipState, UserId,
};
use tracing::{info, warn};

use crate::commands;
use crate::config::Config;
use crate::error::Fatal;
use crate::framework::Handler;
use crate::state::AppState;

/// Бот работает только через взаимодействия и не читает сообщения. `GUILDS` наполняет кэш
/// серверов, ролей и каналов — этого достаточно для проверки прав и `/staff`.
const INTENTS: GatewayIntents = GatewayIntents::GUILDS;

pub async fn run() -> Result<(), Fatal> {
    let config = Config::from_env()?;
    let http = Http::new(&config.token);

    // Первый запрос заодно проверяет токен: ошибка здесь понятнее, чем отказ Gateway позже.
    let app = http
        .get_current_application_info()
        .await
        .map_err(Fatal::context(
            "не удалось получить данные приложения: проверьте DISCORD_TOKEN и доступ к discord.com",
        ))?;
    http.set_application_id(app.id);
    info!("Приложение «{}» (ID: {})", app.name, app.id);

    let developers = developers(config.developers, &app);
    info!("Разработчиков бота: {}", developers.len());

    // Регистрация до подключения к Gateway: к первому взаимодействию команды уже актуальны, а
    // ошибка в определении команды останавливает запуск, а не всплывает в работающем боте.
    // Это полная перезапись: команды, удалённые из кода, исчезают из Discord.
    let registry = commands::registry();
    let registered = Command::set_global_commands(&http, registry.definitions())
        .await
        .map_err(Fatal::context("не удалось зарегистрировать слэш-команды"))?;
    info!("Зарегистрировано слэш-команд: {}", registered.len());

    let state = AppState::new(developers);
    let mut client = ClientBuilder::new_with_http(http, INTENTS)
        .event_handler(Handler::new(state.clone(), registry))
        .await
        .map_err(Fatal::context("не удалось создать клиент Discord"))?;
    state.set_shard_manager(client.shard_manager.clone());

    // Корректное завершение: закрываем соединения с Gateway, и `start` возвращается.
    let shard_manager = client.shard_manager.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Получен сигнал завершения, отключаюсь от Discord…");
        shard_manager.shutdown_all().await;
    });

    info!("Подключение к Gateway…");
    client
        .start()
        .await
        .map_err(Fatal::context("соединение с Gateway прервано"))
}

/// Разработчики бота = `BOT_DEVELOPERS` ∪ участники команды приложения в Developer Portal
/// (или его владелец, если приложение не принадлежит команде).
fn developers(mut developers: BTreeSet<UserId>, app: &CurrentApplicationInfo) -> BTreeSet<UserId> {
    match &app.team {
        Some(team) => developers.extend(
            team.members
                .iter()
                .filter(|member| member.membership_state == MembershipState::Accepted)
                .map(|member| member.user.id),
        ),
        None => developers.extend(app.owner.as_ref().map(|owner| owner.id)),
    }
    developers
}

/// Ctrl+C, а на Unix ещё и SIGTERM — его посылают systemd и Docker при остановке.
async fn shutdown_signal() {
    let interrupt = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!("Не удалось подписаться на Ctrl+C: {e}");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(e) => {
                warn!("Не удалось подписаться на SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}
