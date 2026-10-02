//! Контракт слэш-команды и реестр команд.

use std::sync::Arc;

use serenity::all::*;

use super::Caller;
use crate::error::{AppError, Result};
use crate::state::AppState;

/// Слэш-команда вместе с её кнопками и формами.
///
/// Имя, описание, опции и требуемое право — данные; из них [`Registry`] строит определение для
/// Discord, а [`super::Handler`] — проверку доступа. Поэтому команда не может объявить одно
/// право в Discord и забыть проверить его в коде.
#[async_trait]
pub trait SlashCommand: Sync {
    /// Имя команды. Оно же — первый сегмент `custom_id` её кнопок и форм (см. [`super::CustomId`]).
    fn name(&self) -> &'static str;

    fn description(&self) -> &'static str;

    fn options(&self) -> Vec<CreateCommandOption> {
        Vec::new()
    }

    /// Право, без которого команда недоступна; пустое — доступна всем.
    ///
    /// Discord скрывает команду от участников без этого права, а `Handler` проверяет его перед
    /// каждым шагом — командой, кнопкой, формой: у компонентов нет собственной защиты со стороны
    /// Discord. Сервер может настроить доступ в «Настройки сервера → Интеграции», но выдать
    /// команду тому, у кого нет этого права, не получится: бот откажет.
    fn permission(&self) -> Permissions {
        Permissions::empty()
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()>;

    /// Кнопка или меню, чей `custom_id` начинается с имени этой команды.
    async fn component(&self, _cx: Cx<'_>, _component: &ComponentInteraction) -> Result<()> {
        Err(AppError::stale())
    }

    /// Форма, чей `custom_id` начинается с имени этой команды.
    async fn modal(&self, _cx: Cx<'_>, _modal: &ModalInteraction) -> Result<()> {
        Err(AppError::stale())
    }
}

/// Окружение обработчика.
///
/// Реализует `CacheHttp`, поэтому передаётся в методы serenity вместо `Context`:
/// `command.create_response(cx, …)`.
#[derive(Clone, Copy)]
pub struct Cx<'a> {
    pub ctx: &'a Context,
    pub state: &'a AppState,
    /// Вызвавший участник. Право команды ([`SlashCommand::permission`]) у него уже проверено.
    pub caller: Caller<'a>,
}

impl<'a> Cx<'a> {
    /// Сервер вызывающего из кэша.
    pub fn guild(&self) -> Result<GuildRef<'a>> {
        self.caller.guild(&self.ctx.cache)
    }

    /// См. [`Caller::require_post`].
    pub fn require_post(&self, target: ChannelId, extra: Permissions) -> Result<Permissions> {
        self.caller.require_post(&self.ctx.cache, target, extra)
    }
}

impl CacheHttp for Cx<'_> {
    fn http(&self) -> &Http {
        &self.ctx.http
    }

    fn cache(&self) -> Option<&Arc<Cache>> {
        Some(&self.ctx.cache)
    }
}

impl AsRef<Http> for Cx<'_> {
    fn as_ref(&self) -> &Http {
        &self.ctx.http
    }
}

/// Неизменяемый набор команд бота.
#[derive(Clone, Copy)]
pub struct Registry(&'static [&'static dyn SlashCommand]);

impl Registry {
    pub const fn new(commands: &'static [&'static dyn SlashCommand]) -> Self {
        Self(commands)
    }

    /// Поиск по имени. Команд единицы, поэтому линейный поиск быстрее хеширования.
    pub fn get(&self, name: &str) -> Option<&'static dyn SlashCommand> {
        self.0
            .iter()
            .copied()
            .find(|command| command.name() == name)
    }

    /// Определения всех команд для регистрации в Discord.
    pub fn definitions(&self) -> Vec<CreateCommand> {
        self.0.iter().map(|command| definition(*command)).collect()
    }
}

fn definition(command: &dyn SlashCommand) -> CreateCommand {
    let definition = CreateCommand::new(command.name())
        .description(command.description())
        .set_options(command.options())
        // Только серверы: права, роли и каналы, с которыми работает бот, — понятия сервера.
        .contexts(vec![InteractionContext::Guild]);

    let permission = command.permission();
    if permission.is_empty() {
        definition
    } else {
        definition.default_member_permissions(permission)
    }
}
