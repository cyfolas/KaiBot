//! Запуск бота: от переменных окружения до работающего соединения с Gateway.

use std::collections::BTreeSet;

use serenity::all::{
    ApplicationFlags, ClientBuilder, Command, CurrentApplicationInfo, GatewayIntents, Http,
    MembershipState, UserId,
};
use tracing::{info, warn};

use crate::commands;
use crate::config::Config;
use crate::error::Fatal;
use crate::framework::Handler;
use crate::journal::Watcher;
use crate::settings::Settings;
use crate::state::AppState;
use crate::storage::Storage;

/// Интенты, не требующие разрешения Discord:
/// * `GUILDS` — кэш серверов, ролей и каналов: права, иерархия, `/staff`;
/// * `GUILD_MESSAGES` — создание, изменение и удаление сообщений для журнала сообщений;
/// * `GUILD_VOICE_STATES` — входы и выходы в голосовых каналах для их журнала.
const BASE_INTENTS: GatewayIntents = GatewayIntents::GUILDS
    .union(GatewayIntents::GUILD_MESSAGES)
    .union(GatewayIntents::GUILD_VOICE_STATES);

pub async fn run() -> Result<(), Fatal> {
    let config = Config::from_env()?;

    // База — до Discord: без настроек бот не может проверять доступ.
    let storage = Storage::open(&config.database).await?;
    let settings = Settings::load(storage).await?;
    info!("База настроек: {}", config.database.display());

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

    let intents = intents(app.flags.unwrap_or(ApplicationFlags::empty()));
    if !intents.contains(GatewayIntents::MESSAGE_CONTENT) {
        warn!(
            "Message Content Intent выключен в Developer Portal → Bot: журнал сообщений будет \
             записывать удаления без текста и не увидит изменений."
        );
    }
    if !intents.contains(GatewayIntents::GUILD_MEMBERS) {
        warn!(
            "Server Members Intent выключен в Developer Portal → Bot: /staff не получит список \
             участников, а изменения ролей самого бота станут заметны с задержкой."
        );
    }

    let state = AppState::new(developers, intents, settings);
    let mut client = ClientBuilder::new_with_http(http, intents)
        .event_handler(Handler::new(state.clone(), registry))
        .event_handler(Watcher::new(state.clone()))
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

/// Привилегированные интенты запрашиваются, только если включены в Developer Portal: иначе
/// Gateway закрыл бы соединение (код 4014), и бот не запустился бы вовсе. Флаги `*_LIMITED`
/// означают то же разрешение для приложения без верификации.
fn intents(flags: ApplicationFlags) -> GatewayIntents {
    let mut intents = BASE_INTENTS;
    if flags.intersects(
        ApplicationFlags::GATEWAY_MESSAGE_CONTENT
            | ApplicationFlags::GATEWAY_MESSAGE_CONTENT_LIMITED,
    ) {
        intents |= GatewayIntents::MESSAGE_CONTENT;
    }
    if flags.intersects(
        ApplicationFlags::GATEWAY_GUILD_MEMBERS | ApplicationFlags::GATEWAY_GUILD_MEMBERS_LIMITED,
    ) {
        intents |= GatewayIntents::GUILD_MEMBERS;
    }
    intents
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privileged_intents_only_when_allowed() {
        assert_eq!(intents(ApplicationFlags::empty()), BASE_INTENTS);
        assert!(
            intents(ApplicationFlags::GATEWAY_MESSAGE_CONTENT_LIMITED)
                .contains(GatewayIntents::MESSAGE_CONTENT)
        );
        let both = intents(
            ApplicationFlags::GATEWAY_MESSAGE_CONTENT | ApplicationFlags::GATEWAY_GUILD_MEMBERS,
        );
        assert!(both.contains(GatewayIntents::MESSAGE_CONTENT | GatewayIntents::GUILD_MEMBERS));
        assert!(!both.contains(GatewayIntents::GUILD_PRESENCES));
    }
}
