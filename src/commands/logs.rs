//! `/logs` — каналы журналов сервера: сообщения, голосовые каналы, системный.
//!
//! Канал проверяется при выборе, а не при первой записи:
//! * бот должен иметь право писать в него;
//! * вызывающий должен его видеть;
//! * журнал сообщений и голоса может включить только тот, кто сам видит все каналы, которые
//!   охватит журнал: иначе бот стал бы «доверенным посредником» и открыл бы ему содержимое
//!   закрытых каналов;
//! * журнал сообщений ведётся только в канале, закрытом для @everyone: иначе тексты удалённых
//!   сообщений из закрытых каналов стали бы видны всем.

use serenity::all::*;

use crate::domain::text::{describe_permissions, join_within};
use crate::error::{AppError, Result};
use crate::framework::{Cx, Options, SlashCommand};
use crate::journal::{self, system};
use crate::settings::{LogChannels, LogKind};

const NAME: &str = "logs";

/// Права бота в канале любого журнала.
const BOT_BASE: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::SEND_MESSAGES)
    .union(Permissions::EMBED_LINKS);

pub struct Logs;

#[async_trait]
impl SlashCommand for Logs {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Журналы сервера: сообщения, голосовые каналы, системный"
    }

    fn permission(&self) -> Permissions {
        Permissions::MANAGE_GUILD
    }

    fn options(&self) -> Vec<CreateCommandOption> {
        let kind = || {
            let mut option = CreateCommandOption::new(CommandOptionType::String, "type", "Журнал")
                .required(true);
            for kind in LogKind::ALL {
                option = option.add_string_choice(kind.label(), kind.key());
            }
            option
        };

        vec![
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "status",
                "Каналы журналов и права бота в них",
            ),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "set",
                "Выбрать канал журнала",
            )
            .add_sub_option(kind())
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::Channel, "channel", "Канал журнала")
                    .channel_types(vec![ChannelType::Text, ChannelType::News])
                    .required(true),
            ),
            CreateCommandOption::new(CommandOptionType::SubCommand, "disable", "Выключить журнал")
                .add_sub_option(kind()),
        ]
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        let (subcommand, options) = Options::of(command)
            .subcommand()
            .ok_or_else(AppError::stale)?;
        // Права бота читаются из Discord: это может не уложиться в 3 секунды на ответ.
        command.defer_ephemeral(cx).await?;

        let reply = match subcommand {
            "status" => status(cx).await?,
            "set" => set(cx, &options).await?,
            "disable" => disable(cx, &options).await?,
            _ => return Err(AppError::stale()),
        };
        command
            .edit_response(cx, EditInteractionResponse::new().content(reply))
            .await?;
        Ok(())
    }
}

fn kind(options: &Options<'_>) -> Result<LogKind> {
    options
        .str("type")
        .and_then(LogKind::from_key)
        .ok_or_else(AppError::stale)
}

/// Права, которые нужны боту в канале журнала.
fn bot_requires(kind: LogKind) -> Permissions {
    match kind {
        // Расшифровка массового удаления прикладывается файлом.
        LogKind::Messages => BOT_BASE | Permissions::ATTACH_FILES,
        LogKind::Voice | LogKind::System => BOT_BASE,
    }
}

/// Каналы, события которых попадают в журнал.
fn watched(kind: LogKind, channel: ChannelType) -> bool {
    match kind {
        // Сообщения пишут и в текстовых чатах голосовых каналов, и в ветках форумов.
        LogKind::Messages => channel != ChannelType::Category,
        LogKind::Voice => matches!(channel, ChannelType::Voice | ChannelType::Stage),
        LogKind::System => false,
    }
}

/// Что известно о канале на сервере: права бота, видимость для @everyone и каналы, которые
/// журнал охватит, а вызывающий не видит.
struct ChannelCheck {
    bot_missing: Permissions,
    public: bool,
    hidden_from_caller: Vec<ChannelId>,
}

fn check_channel(
    cx: Cx<'_>,
    bot: &Member,
    channel: ChannelId,
    kind: LogKind,
) -> Result<ChannelCheck> {
    let guild = cx.guild()?;
    let channel = guild
        .channels
        .get(&channel)
        .ok_or_else(|| AppError::user("Канал не найден на этом сервере."))?;

    let hidden_from_caller = guild
        .channels
        .values()
        .filter(|watched_channel| watched(kind, watched_channel.kind))
        .filter(|watched_channel| {
            guild
                .user_permissions_in(watched_channel, bot)
                .view_channel()
        })
        .filter(|watched_channel| {
            !guild
                .user_permissions_in(watched_channel, cx.caller.member)
                .view_channel()
        })
        .map(|watched_channel| watched_channel.id)
        .collect();

    Ok(ChannelCheck {
        bot_missing: bot_requires(kind) - guild.user_permissions_in(channel, bot),
        public: journal::is_public(&guild, channel),
        hidden_from_caller,
    })
}

async fn set(cx: Cx<'_>, options: &Options<'_>) -> Result<String> {
    let kind = kind(options)?;
    let channel = options.channel("channel").ok_or_else(AppError::stale)?;

    // Настраивающий сам должен видеть журнал: иначе он направил бы данные туда, куда не видит.
    if !cx.permissions_in(channel)?.view_channel() {
        return Err(AppError::user(format!(
            "Вы не видите {} — выберите канал, доступный вам.",
            channel.mention()
        )));
    }
    let check = check_channel(cx, &cx.bot_member().await?, channel, kind)?;
    if !check.bot_missing.is_empty() {
        return Err(AppError::user(format!(
            "Боту не хватает прав в {}: {}.",
            channel.mention(),
            describe_permissions(check.bot_missing)
        )));
    }
    if !check.hidden_from_caller.is_empty() {
        let mentions: Vec<String> = check
            .hidden_from_caller
            .iter()
            .map(|channel| channel.mention().to_string())
            .collect();
        return Err(AppError::user(format!(
            "Журнал «{}» охватит каналы, которых вы не видите: {}. Включить его может тот, кто \
             видит их все (например, администратор).",
            kind.label(),
            join_within(&mentions, 1000)
        )));
    }
    if kind == LogKind::Messages && check.public {
        return Err(AppError::user(format!(
            "{} виден всем участникам (@everyone): тексты удалённых сообщений из закрытых каналов \
             стали бы видны всем. Выберите канал, закрытый для @everyone.",
            channel.mention()
        )));
    }

    let before = cx
        .state
        .settings
        .update_guild(cx.caller.guild_id, |settings| {
            Ok(settings.logs.set(kind, Some(channel)))
        })
        .await?;
    if before == Some(channel) {
        return Ok(format!(
            "Журнал «{}» уже ведётся в {}.",
            kind.label(),
            channel.mention()
        ));
    }
    // Канал журнала сам не журналируется: запомненные из него сообщения больше не нужны.
    cx.state.journal.forget_channel(channel);

    let author = &cx.caller.member.user;
    journal::post(
        &cx.ctx.http,
        cx.state,
        cx.caller.guild_id,
        kind,
        CreateMessage::new().content(format!(
            "📒 Здесь ведётся журнал «{}» (настройка: {}).",
            kind.label(),
            author.mention()
        )),
    )
    .await;
    journal::system(
        cx,
        system::setting_changed(
            author,
            &format!("Журнал «{}»", kind.label()),
            &describe_channel(before),
            &describe_channel(Some(channel)),
        ),
    )
    .await;

    let mut reply = format!(
        "✅ Журнал «{}» ведётся в {}.",
        kind.label(),
        channel.mention()
    );
    if kind == LogKind::Messages && !content_intent(cx) {
        reply += "\n⚠️ Message Content Intent выключен: в журнале будут удаления, но без текста \
                  сообщений, а изменения записываться не будут. Включите его в Developer Portal → \
                  Bot и перезапустите бота.";
    }
    Ok(reply)
}

async fn disable(cx: Cx<'_>, options: &Options<'_>) -> Result<String> {
    let kind = kind(options)?;
    let before = cx
        .state
        .settings
        .update_guild(cx.caller.guild_id, |settings| {
            Ok(settings.logs.set(kind, None))
        })
        .await?;
    let Some(channel) = before else {
        return Ok(format!("Журнал «{}» и так выключен.", kind.label()));
    };
    if kind == LogKind::Messages {
        cx.state.journal.forget_messages(cx.caller.guild_id);
    }

    journal::system(
        cx,
        system::setting_changed(
            &cx.caller.member.user,
            &format!("Журнал «{}»", kind.label()),
            &describe_channel(Some(channel)),
            &describe_channel(None),
        ),
    )
    .await;
    Ok(format!("✅ Журнал «{}» выключен.", kind.label()))
}

async fn status(cx: Cx<'_>) -> Result<String> {
    let logs: LogChannels = cx.settings().logs;
    let bot = cx.bot_member().await?;
    let mut text = String::from("**Журналы сервера**\n");
    for kind in LogKind::ALL {
        let line = match logs.get(kind) {
            None => "выключен".to_string(),
            Some(channel) => match check_channel(cx, &bot, channel, kind) {
                Ok(check) if !check.bot_missing.is_empty() => format!(
                    "{} ⚠️ боту не хватает прав: {}",
                    channel.mention(),
                    describe_permissions(check.bot_missing)
                ),
                // Права канала могли измениться после выбора.
                Ok(check) if kind == LogKind::Messages && check.public => format!(
                    "{} ⚠️ канал виден всем (@everyone) — закройте его или выберите другой",
                    channel.mention()
                ),
                Ok(_) => format!("{} ✅", channel.mention()),
                Err(AppError::User(problem)) => format!("{} ⚠️ {problem}", channel.mention()),
                Err(err) => return Err(err),
            },
        };
        text += &format!("• {}: {line}\n", kind.label());
    }

    text += &format!(
        "\nMessage Content Intent: {}",
        if content_intent(cx) {
            "включён — в журнале сообщений есть их текст."
        } else {
            "**выключен** — журнал сообщений записывает удаления без текста и не видит изменений."
        }
    );
    text += &format!(
        "\nБот помнит последние {} сообщений каждого канала с момента запуска; о более старых \
         известны только ID и время отправки.",
        crate::journal::MESSAGES_PER_CHANNEL
    );
    Ok(text)
}

fn content_intent(cx: Cx<'_>) -> bool {
    cx.state.intents().contains(GatewayIntents::MESSAGE_CONTENT)
}

fn describe_channel(channel: Option<ChannelId>) -> String {
    channel.map_or_else(|| "выключен".to_string(), |c| c.mention().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_channels_by_log() {
        assert!(watched(LogKind::Messages, ChannelType::Voice));
        assert!(watched(LogKind::Messages, ChannelType::Forum));
        assert!(!watched(LogKind::Messages, ChannelType::Category));
        assert!(watched(LogKind::Voice, ChannelType::Stage));
        assert!(!watched(LogKind::Voice, ChannelType::Text));
        assert!(!watched(LogKind::System, ChannelType::Text));
    }

    #[test]
    fn message_log_needs_attachments() {
        assert!(bot_requires(LogKind::Messages).contains(Permissions::ATTACH_FILES));
        assert!(!bot_requires(LogKind::Voice).contains(Permissions::ATTACH_FILES));
    }
}
