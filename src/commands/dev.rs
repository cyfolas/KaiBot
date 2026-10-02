//! `/dev` — управление ботом на уровне приложения: режим обслуживания и глобальные блокировки.
//!
//! Только для разработчиков (см. [`crate::access`]). Действия разработчиков не привязаны к
//! серверу, поэтому пишутся в журнал процесса с target `audit`, а не в системный журнал сервера.

use serenity::all::*;
use tracing::info;

use crate::access::GlobalMode;
use crate::error::{AppError, Result};
use crate::framework::{Cx, Options, SlashCommand, ephemeral, ephemeral_embed};

const NAME: &str = "dev";

pub struct Dev;

#[async_trait]
impl SlashCommand for Dev {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Для разработчиков: режим бота и глобальные блокировки"
    }

    fn developer_only(&self) -> bool {
        true
    }

    fn options(&self) -> Vec<CreateCommandOption> {
        let user = |description| {
            CreateCommandOption::new(CommandOptionType::User, "user", description).required(true)
        };
        let mut mode = CreateCommandOption::new(CommandOptionType::String, "mode", "Новый режим")
            .required(true);
        for value in GlobalMode::ALL {
            mode = mode.add_string_choice(value.label(), value.key());
        }

        vec![
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "status",
                "Состояние бота: режим, блокировки, серверы",
            ),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "mode",
                "Сменить режим: публичный или только для разработчиков",
            )
            .add_sub_option(mode),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "block",
                "Закрыть пользователю доступ к боту на всех серверах",
            )
            .add_sub_option(user("Кого заблокировать")),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "unblock",
                "Вернуть пользователю доступ к боту",
            )
            .add_sub_option(user("Кого разблокировать")),
        ]
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        let (subcommand, options) = Options::of(command)
            .subcommand()
            .ok_or_else(AppError::stale)?;
        let author = &cx.caller.member.user;

        let response = match subcommand {
            "status" => ephemeral_embed(status(cx)),
            "mode" => {
                let mode = options
                    .str("mode")
                    .and_then(GlobalMode::from_key)
                    .ok_or_else(AppError::stale)?;
                let before = cx
                    .state
                    .settings
                    .update_global(|global| Ok(std::mem::replace(&mut global.mode, mode)))
                    .await?;
                if before == mode {
                    ephemeral(format!("Режим уже «{}».", mode.label()))
                } else {
                    info!(target: "audit", "/dev mode: {} ({}) → {}", author.tag(), author.id, mode.key());
                    ephemeral(format!("✅ Режим бота: «{}».", mode.label()))
                }
            }
            "block" | "unblock" => {
                let (user, _) = options.user("user").ok_or_else(AppError::stale)?;
                let block = subcommand == "block";
                if block && cx.state.is_developer(user.id) {
                    return Err(AppError::user("Разработчика заблокировать нельзя."));
                }
                let changed = cx
                    .state
                    .settings
                    .update_global(|global| {
                        Ok(if block {
                            global.blocked.insert(user.id)
                        } else {
                            global.blocked.remove(&user.id)
                        })
                    })
                    .await?;

                let mention = user.mention();
                ephemeral(match (block, changed) {
                    (true, true) => {
                        info!(target: "audit", "/dev block: {} ({}) → {}", author.tag(), author.id, user.id);
                        format!("✅ Доступ к боту для {mention} закрыт на всех серверах.")
                    }
                    (false, true) => {
                        info!(target: "audit", "/dev unblock: {} ({}) → {}", author.tag(), author.id, user.id);
                        format!("✅ Доступ к боту для {mention} восстановлен.")
                    }
                    (true, false) => format!("Доступ для {mention} уже закрыт."),
                    (false, false) => format!("Доступ для {mention} и не был закрыт."),
                })
            }
            _ => return Err(AppError::stale()),
        };
        command.create_response(cx, response).await?;
        Ok(())
    }
}

fn status(cx: Cx<'_>) -> CreateEmbed {
    let global = cx.state.settings.global();
    let intents = cx.state.intents();
    let flag = |enabled: bool| if enabled { "вкл." } else { "выкл." };

    CreateEmbed::new()
        .colour(Colour::new(0x0058_65F2))
        .title(format!("{} · уровень приложения", crate::BOT_NAME))
        .field("Режим", global.mode.label(), true)
        .field("Заблокировано", global.blocked.len().to_string(), true)
        .field(
            "Разработчиков",
            cx.state.developers().len().to_string(),
            true,
        )
        .field("Серверов", cx.ctx.cache.guild_count().to_string(), true)
        .field(
            "С настройками",
            cx.state.settings.configured_guilds().to_string(),
            true,
        )
        .field(
            "Интенты",
            format!(
                "Message Content: {}\nServer Members: {}",
                flag(intents.contains(GatewayIntents::MESSAGE_CONTENT)),
                flag(intents.contains(GatewayIntents::GUILD_MEMBERS)),
            ),
            true,
        )
        .footer(CreateEmbedFooter::new(format!(
            "{} v{}",
            crate::BOT_NAME,
            env!("CARGO_PKG_VERSION")
        )))
}
