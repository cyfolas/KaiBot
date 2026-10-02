//! Записи системного журнала: изменения настроек и доступа, состава администрации, публикации
//! от имени бота, положение роли бота, неполадки журналов.
//!
//! В Discord автором действий бота значится бот; реальный инициатор виден только здесь.

use serenity::all::*;

use crate::hierarchy::{Standing, mentions};
use crate::settings::LogKind;

const COLOR_INFO: Colour = Colour(0x0058_65F2);
const COLOR_STAFF: Colour = Colour(0x00EB_459E);
const COLOR_WARNING: Colour = Colour(0x00FE_E75C);

fn base(colour: Colour, title: &str, actor: Option<&User>) -> CreateEmbed {
    let embed = CreateEmbed::new()
        .colour(colour)
        .title(title)
        .timestamp(Timestamp::now());
    match actor {
        Some(actor) => embed
            .field("Кем", format!("{} ({})", actor.mention(), actor.name), true)
            .footer(CreateEmbedFooter::new(format!("ID: {}", actor.id))),
        None => embed,
    }
}

/// Изменение настройки: что было и что стало.
pub fn setting_changed(actor: &User, title: &str, before: &str, after: &str) -> CreateEmbed {
    base(COLOR_INFO, &format!("⚙️ {title}"), Some(actor))
        .field("Было", before, true)
        .field("Стало", after, true)
}

/// Изменение состава администрации.
pub fn staff_changed(
    actor: &User,
    target: UserId,
    added: Option<RoleId>,
    removed: &[RoleId],
    reason: Option<&str>,
) -> CreateEmbed {
    let mut embed = base(COLOR_STAFF, "🛡️ Состав администрации изменён", Some(actor)).field(
        "Участник",
        target.mention().to_string(),
        true,
    );
    if let Some(role) = added {
        embed = embed.field("Выдана роль", role.mention().to_string(), true);
    }
    if !removed.is_empty() {
        embed = embed.field("Сняты роли", mentions(removed), true);
    }
    if let Some(reason) = reason {
        embed = embed.field("Причина", crate::text::truncate(reason, 1024), false);
    }
    embed
}

/// Публикация от имени бота: в Discord автором будет бот.
pub fn published(command: &str, author: &User, channel: ChannelId, link: &str) -> CreateEmbed {
    base(
        COLOR_INFO,
        &format!("📢 Публикация от имени бота · /{command}"),
        Some(author),
    )
    .field("Канал", channel.mention().to_string(), true)
    .field("Сообщение", format!("[перейти]({link})"), true)
}

/// Положение роли бота изменилось: что он теперь может.
pub fn standing_changed(standing: &Standing) -> CreateEmbed {
    let colour = if standing.manage_roles && standing.unmanageable.is_empty() {
        COLOR_INFO
    } else {
        COLOR_WARNING
    };
    base(colour, "🧭 Положение роли бота изменилось", None).description(standing.describe())
}

/// Журнал недоступен: бот не может писать в его канал.
pub fn log_unavailable(kind: LogKind, channel: ChannelId, reason: &str) -> CreateEmbed {
    base(COLOR_WARNING, "⚠️ Журнал недоступен", None).description(format!(
        "Не удаётся писать в журнал «{}» ({}): {reason}\n\
         Проверьте права бота в канале или выберите другой: `/logs set`.",
        kind.label(),
        channel.mention()
    ))
}

/// Канал журнала удалён — журнал выключен.
pub fn log_channel_deleted(kinds: &[LogKind], channel: ChannelId) -> CreateEmbed {
    let names: Vec<String> = kinds.iter().map(|k| format!("«{}»", k.label())).collect();
    base(COLOR_WARNING, "⚠️ Журнал выключен", None).description(format!(
        "Канал {} удалён, поэтому журналы {} выключены. Выберите новый канал: `/logs set`.",
        channel.mention(),
        names.join(", ")
    ))
}
