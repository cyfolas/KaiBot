//! События Gateway: маршрутизация взаимодействий к командам, проверка доступа, отчёт об ошибках.

use std::sync::Arc;

use serenity::all::*;
use tracing::{debug, error, info, warn};

use super::reply::report_error;
use super::{Caller, CustomId, Cx, Registry, SlashCommand};
use crate::access::{self, Principal};
use crate::error::{AppError, Chain, Result};
use crate::state::AppState;

pub struct Handler {
    state: Arc<AppState>,
    registry: Registry,
}

impl Handler {
    pub fn new(state: Arc<AppState>, registry: Registry) -> Self {
        Self { state, registry }
    }

    /// Команда-владелец взаимодействия: по имени для слэш-команд, по первому сегменту
    /// `custom_id` для кнопок и форм.
    fn owner(&self, name: &str) -> Result<&'static dyn SlashCommand> {
        self.registry.get(name).ok_or_else(AppError::stale)
    }

    /// Окружение обработчика. Здесь — единственная точка проверки доступа к боту и права
    /// команды: они проверяются на каждом шаге, а не только при вызове слэш-команды. Поэтому
    /// смена режима или отзыв доступа действуют и на уже открытые формы и кнопки.
    fn cx<'a>(
        &'a self,
        ctx: &'a Context,
        command: &dyn SlashCommand,
        caller: Caller<'a>,
    ) -> Result<Cx<'a>> {
        let principal = Principal {
            user: caller.member.user.id,
            roles: &caller.member.roles,
            developer: self.state.is_developer(caller.member.user.id),
            permissions: caller.permissions(),
        };
        access::check(
            &self.state.settings.global(),
            &self.state.settings.guild(caller.guild_id).access,
            &principal,
            command.developer_only(),
        )
        .map_err(|denial| AppError::user(denial.message()))?;
        caller.require(command.permission())?;
        Ok(Cx {
            ctx,
            state: &self.state,
            caller,
        })
    }

    async fn on_command(&self, ctx: &Context, interaction: &CommandInteraction) -> Result<()> {
        let command = self.owner(&interaction.data.name)?;
        let caller = Caller::new(
            interaction.guild_id,
            interaction.member.as_deref(),
            interaction.channel_id,
            interaction.channel.as_ref(),
        )?;
        command
            .run(self.cx(ctx, command, caller)?, interaction)
            .await
    }

    async fn on_component(&self, ctx: &Context, interaction: &ComponentInteraction) -> Result<()> {
        let command = self.owner(CustomId::owner(&interaction.data.custom_id))?;
        let caller = Caller::new(
            interaction.guild_id,
            interaction.member.as_ref(),
            interaction.channel_id,
            interaction.channel.as_ref(),
        )?;
        command
            .component(self.cx(ctx, command, caller)?, interaction)
            .await
    }

    async fn on_modal(&self, ctx: &Context, interaction: &ModalInteraction) -> Result<()> {
        let command = self.owner(CustomId::owner(&interaction.data.custom_id))?;
        let caller = Caller::new(
            interaction.guild_id,
            interaction.member.as_ref(),
            interaction.channel_id,
            interaction.channel.as_ref(),
        )?;
        command
            .modal(self.cx(ctx, command, caller)?, interaction)
            .await
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!(
            "Подключён к Gateway как {} (ID: {}), серверов: {}",
            ready.user.tag(),
            ready.user.id,
            ready.guilds.len()
        );
        // Имя аккаунта задаётся в Developer Portal; Discord ограничивает частоту его смены,
        // поэтому бот не меняет его сам, а только напоминает.
        if ready.user.name != crate::BOT_NAME {
            warn!(
                "Имя аккаунта бота «{}» отличается от «{}»: переименуйте его в Developer Portal → Bot.",
                ready.user.name,
                crate::BOT_NAME
            );
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let (outcome, id, token, label) = match &interaction {
            Interaction::Command(i) => {
                (self.on_command(&ctx, i).await, i.id, &i.token, &i.data.name)
            }
            Interaction::Component(i) => (
                self.on_component(&ctx, i).await,
                i.id,
                &i.token,
                &i.data.custom_id,
            ),
            Interaction::Modal(i) => (
                self.on_modal(&ctx, i).await,
                i.id,
                &i.token,
                &i.data.custom_id,
            ),
            _ => return,
        };

        if let Err(err) = outcome {
            match &err {
                AppError::User(message) => debug!("{label}: отказ пользователю: {message}"),
                AppError::Discord(_) | AppError::Storage(_) => error!("{label}: {}", Chain(&err)),
            }
            report_error(&ctx, id, token, &err).await;
        }
    }
}
