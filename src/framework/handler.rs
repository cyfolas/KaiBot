//! События Gateway: маршрутизация взаимодействий к командам, проверка доступа, отчёт об ошибках.

use std::sync::Arc;

use serenity::all::*;
use tracing::{debug, error, info};

use super::reply::report_error;
use super::{Caller, CustomId, Cx, Registry, SlashCommand};
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

    /// Окружение обработчика. Здесь — единственная точка проверки права команды: оно
    /// проверяется на каждом шаге, а не только при вызове слэш-команды.
    fn cx<'a>(
        &'a self,
        ctx: &'a Context,
        command: &dyn SlashCommand,
        caller: Caller<'a>,
    ) -> Result<Cx<'a>> {
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
                AppError::Discord(e) => error!("{label}: {}", Chain(e.as_ref())),
            }
            report_error(&ctx, id, token, &err).await;
        }
    }
}
