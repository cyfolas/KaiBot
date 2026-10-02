//! Журнал сообщений: память о недавних сообщениях и записи об их изменении и удалении.
//!
//! Discord сообщает об удалении только ID, поэтому бот сам помнит последние сообщения — только
//! на серверах, где журнал сообщений включён, и только то, что нужно для записи (автор, текст,
//! имена вложений). Объём ограничен [`PER_CHANNEL`] сообщениями на канал; память не переживает
//! перезапуск. Сообщения ботов и вебхуков в журнал не попадают, но помнятся (без текста), чтобы
//! их удаление не выглядело удалением неизвестного сообщения.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};

use serenity::all::*;

use crate::text::truncate;

/// Сколько последних сообщений канала помнит бот.
pub const PER_CHANNEL: usize = 200;

/// Лимит описания embed — 4096; две части записи об изменении и подписи укладываются в него.
const PART_MAX: usize = 1900;
/// Изменение сообщения, которого бот не помнит, записывается, только если оно свежее: иначе
/// это обновление превью ссылок или другое служебное обновление давно изменённого сообщения.
const FRESH_EDIT_SECS: i64 = 300;

const COLOR_EDITED: Colour = Colour(0x00FE_E75C);
const COLOR_DELETED: Colour = Colour(0x00ED_4245);

/// То, что бот помнит о сообщении.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub id: MessageId,
    pub author: UserId,
    pub author_name: String,
    /// Сообщение бота или вебхука.
    pub automated: bool,
    pub content: String,
    pub attachments: Vec<String>,
}

impl Snapshot {
    pub fn of(message: &Message) -> Self {
        let automated = message.author.bot || message.webhook_id.is_some();
        Self {
            id: message.id,
            author: message.author.id,
            author_name: message.author.name.clone(),
            automated,
            content: if automated {
                String::new()
            } else {
                message.content.clone()
            },
            attachments: if automated {
                Vec::new()
            } else {
                attachment_names(&message.attachments)
            },
        }
    }
}

pub fn attachment_names(attachments: &[Attachment]) -> Vec<String> {
    attachments.iter().map(|a| a.filename.clone()).collect()
}

/// Итог сравнения нового состояния сообщения с запомненным.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    /// Текст или вложения изменились; прежнее состояние.
    Changed(Snapshot),
    /// Ничего значимого не изменилось (превью ссылок, закрепление) или сообщение автоматическое.
    Unchanged,
    /// Сообщения нет в памяти.
    Unknown,
}

#[derive(Default)]
pub struct MessageStore {
    channels: Mutex<HashMap<ChannelId, Channel>>,
}

struct Channel {
    guild: GuildId,
    messages: VecDeque<Snapshot>,
}

impl MessageStore {
    pub fn remember(&self, guild: GuildId, channel: ChannelId, snapshot: Snapshot) {
        let mut channels = self.lock();
        let entry = channels.entry(channel).or_insert_with(|| Channel {
            guild,
            messages: VecDeque::with_capacity(PER_CHANNEL),
        });
        if entry.messages.len() >= PER_CHANNEL {
            entry.messages.pop_front();
        }
        entry.messages.push_back(snapshot);
    }

    /// Сравнивает новое состояние с запомненным и запоминает новое. `attachments: None` —
    /// Discord не прислал вложения, значит они не менялись.
    pub fn edit(
        &self,
        channel: ChannelId,
        id: MessageId,
        content: &str,
        attachments: Option<Vec<String>>,
    ) -> Edit {
        let mut channels = self.lock();
        let Some(stored) = channels
            .get_mut(&channel)
            .and_then(|c| c.messages.iter_mut().find(|m| m.id == id))
        else {
            return Edit::Unknown;
        };
        let attachments = attachments.unwrap_or_else(|| stored.attachments.clone());
        if stored.automated || (stored.content == content && stored.attachments == attachments) {
            return Edit::Unchanged;
        }
        let before = stored.clone();
        stored.content = content.to_string();
        stored.attachments = attachments;
        Edit::Changed(before)
    }

    pub fn take(&self, channel: ChannelId, id: MessageId) -> Option<Snapshot> {
        let mut channels = self.lock();
        let messages = &mut channels.get_mut(&channel)?.messages;
        let index = messages.iter().position(|m| m.id == id)?;
        messages.remove(index)
    }

    /// Забирает запомненные сообщения из `ids` в порядке отправки.
    pub fn take_many(&self, channel: ChannelId, ids: &[MessageId]) -> Vec<Snapshot> {
        let mut channels = self.lock();
        let Some(entry) = channels.get_mut(&channel) else {
            return Vec::new();
        };
        let (taken, kept): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut entry.messages)
            .into_iter()
            .partition(|m| ids.contains(&m.id));
        entry.messages = kept;
        taken.into()
    }

    pub fn forget_channel(&self, channel: ChannelId) {
        self.lock().remove(&channel);
    }

    pub fn forget_guild(&self, guild: GuildId) {
        self.lock().retain(|_, channel| channel.guild != guild);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ChannelId, Channel>> {
        self.channels.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Свежее ли изменение (см. [`FRESH_EDIT_SECS`]). Модуль разницы — на случай расхождения часов.
pub fn is_fresh_edit(edited_at: Option<Timestamp>, now: Timestamp) -> bool {
    edited_at
        .is_some_and(|at| (now.unix_timestamp() - at.unix_timestamp()).abs() <= FRESH_EDIT_SECS)
}

/// Где и чьё сообщение.
pub struct Origin<'a> {
    pub guild: GuildId,
    pub channel: ChannelId,
    pub id: MessageId,
    pub author: UserId,
    pub author_name: &'a str,
}

/// Запись об изменении. `before` — `None`, если прежнего текста бот не помнит.
pub fn edited(
    origin: &Origin<'_>,
    before: Option<&Snapshot>,
    content: &str,
    attachments: &[String],
) -> CreateEmbed {
    let before = match before {
        Some(snapshot) => body(&snapshot.content, &snapshot.attachments),
        None => "*неизвестно: сообщение отправлено до запуска бота или давно*".to_string(),
    };
    let description = format!(
        "**Было:**\n{}\n\n**Стало:**\n{}",
        truncate(&before, PART_MAX),
        truncate(&body(content, attachments), PART_MAX)
    );
    CreateEmbed::new()
        .colour(COLOR_EDITED)
        .author(CreateEmbedAuthor::new(format!(
            "Сообщение изменено · {}",
            origin.author_name
        )))
        .description(description)
        .field("Автор", origin.author.mention().to_string(), true)
        .field("Канал", origin.channel.mention().to_string(), true)
        .field(
            "Сообщение",
            format!(
                "[перейти]({})",
                origin.id.link(origin.channel, Some(origin.guild))
            ),
            true,
        )
        .footer(CreateEmbedFooter::new(format!(
            "Автор: {} · Сообщение: {}",
            origin.author, origin.id
        )))
        .timestamp(Timestamp::now())
}

/// Запись об удалении. Без `snapshot` — сообщение, которого бот не помнит.
pub fn deleted(
    channel: ChannelId,
    id: MessageId,
    snapshot: Option<&Snapshot>,
    content_intent: bool,
) -> CreateEmbed {
    let sent = format!("<t:{}:R>", id.created_at().unix_timestamp());
    let embed = CreateEmbed::new()
        .colour(COLOR_DELETED)
        .field("Канал", channel.mention().to_string(), true)
        .field("Отправлено", sent, true)
        .timestamp(Timestamp::now());

    match snapshot {
        Some(snapshot) => {
            let text = if !content_intent && snapshot.content.is_empty() {
                "*текст недоступен: не включён Message Content Intent*".to_string()
            } else {
                body(&snapshot.content, &snapshot.attachments)
            };
            embed
                .author(CreateEmbedAuthor::new(format!(
                    "Сообщение удалено · {}",
                    snapshot.author_name
                )))
                .description(truncate(&text, PART_MAX * 2))
                .field("Автор", snapshot.author.mention().to_string(), true)
                .footer(CreateEmbedFooter::new(format!(
                    "Автор: {} · Сообщение: {id}",
                    snapshot.author
                )))
        }
        None => embed
            .author(CreateEmbedAuthor::new("Сообщение удалено"))
            .description("*Содержимое неизвестно: сообщение отправлено до запуска бота или давно.*")
            .footer(CreateEmbedFooter::new(format!("Сообщение: {id}"))),
    }
}

/// Запись о массовом удалении и расшифровка запомненных сообщений файлом.
pub fn bulk_deleted(
    channel: ChannelId,
    total: usize,
    known: &[Snapshot],
) -> (CreateEmbed, Option<CreateAttachment>) {
    let human: Vec<&Snapshot> = known.iter().filter(|m| !m.automated).collect();
    let embed = CreateEmbed::new()
        .colour(COLOR_DELETED)
        .author(CreateEmbedAuthor::new("Массовое удаление сообщений"))
        .description(format!(
            "Удалено сообщений: **{total}** в {}.\nИзвестно боту: {}{}.",
            channel.mention(),
            known.len(),
            if human.is_empty() {
                ""
            } else {
                " — текст в приложенном файле"
            }
        ))
        .timestamp(Timestamp::now());

    if human.is_empty() {
        return (embed, None);
    }
    let mut transcript = format!(
        "Удалено сообщений: {total}, известно боту: {}.\n",
        known.len()
    );
    for message in human {
        transcript += &format!(
            "\n[{}] {} ({}):\n{}\n",
            message.id.created_at(),
            message.author_name,
            message.author,
            body(&message.content, &message.attachments)
        );
    }
    let file = CreateAttachment::bytes(transcript.into_bytes(), format!("deleted-{channel}.txt"));
    (embed, Some(file))
}

/// Текст сообщения и список вложений.
fn body(content: &str, attachments: &[String]) -> String {
    let mut text = if content.is_empty() {
        "*без текста*".to_string()
    } else {
        content.to_string()
    };
    if !attachments.is_empty() {
        text += &format!("\n📎 {}", attachments.join(", "));
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    const GUILD: GuildId = GuildId::new(1);
    const CHANNEL: ChannelId = ChannelId::new(2);

    fn snapshot(id: u64, content: &str) -> Snapshot {
        Snapshot {
            id: MessageId::new(id),
            author: UserId::new(3),
            author_name: "user".into(),
            automated: false,
            content: content.into(),
            attachments: Vec::new(),
        }
    }

    fn description_len(embed: CreateEmbed) -> usize {
        let json: Value = serde_json::to_value(embed).unwrap();
        json["description"].as_str().unwrap().chars().count()
    }

    #[test]
    fn store_is_bounded_per_channel() {
        let store = MessageStore::default();
        for id in 1..=(PER_CHANNEL as u64 + 10) {
            store.remember(GUILD, CHANNEL, snapshot(id, "x"));
        }
        assert!(
            store.take(CHANNEL, MessageId::new(1)).is_none(),
            "старейшие вытеснены"
        );
        assert!(
            store
                .take(CHANNEL, MessageId::new(PER_CHANNEL as u64 + 10))
                .is_some()
        );
    }

    #[test]
    fn edits_compare_with_remembered_state() {
        let store = MessageStore::default();
        store.remember(GUILD, CHANNEL, snapshot(1, "до"));
        let id = MessageId::new(1);

        assert_eq!(
            store.edit(CHANNEL, id, "до", Some(Vec::new())),
            Edit::Unchanged
        );
        assert_eq!(
            store.edit(CHANNEL, id, "после", Some(Vec::new())),
            Edit::Changed(snapshot(1, "до"))
        );
        // Новое состояние запомнено: повторное обновление (превью ссылки) — не изменение.
        assert_eq!(
            store.edit(CHANNEL, id, "после", Some(Vec::new())),
            Edit::Unchanged
        );
        assert_eq!(
            store.edit(CHANNEL, MessageId::new(9), "x", Some(Vec::new())),
            Edit::Unknown
        );
    }

    #[test]
    fn automated_messages_are_never_logged_as_edited() {
        let store = MessageStore::default();
        store.remember(
            GUILD,
            CHANNEL,
            Snapshot {
                automated: true,
                ..snapshot(1, "")
            },
        );
        assert_eq!(
            store.edit(CHANNEL, MessageId::new(1), "новое", None),
            Edit::Unchanged
        );
    }

    #[test]
    fn take_many_keeps_the_rest() {
        let store = MessageStore::default();
        for id in 1..=4 {
            store.remember(GUILD, CHANNEL, snapshot(id, "x"));
        }
        let taken = store.take_many(CHANNEL, &[MessageId::new(2), MessageId::new(4)]);
        assert_eq!(
            taken.iter().map(|m| m.id.get()).collect::<Vec<_>>(),
            vec![2, 4]
        );
        assert!(store.take(CHANNEL, MessageId::new(3)).is_some());
        assert!(store.take(CHANNEL, MessageId::new(2)).is_none());
    }

    #[test]
    fn forgetting_a_guild_drops_its_channels_only() {
        let store = MessageStore::default();
        store.remember(GUILD, CHANNEL, snapshot(1, "x"));
        store.remember(GuildId::new(5), ChannelId::new(6), snapshot(2, "y"));
        store.forget_guild(GUILD);
        assert!(store.take(CHANNEL, MessageId::new(1)).is_none());
        assert!(store.take(ChannelId::new(6), MessageId::new(2)).is_some());
    }

    #[test]
    fn freshness_window() {
        let now = Timestamp::from_unix_timestamp(1_000_000).unwrap();
        let at = |secs| Some(Timestamp::from_unix_timestamp(secs).unwrap());
        assert!(is_fresh_edit(at(1_000_000 - 60), now));
        assert!(is_fresh_edit(at(1_000_000 + 5), now));
        assert!(!is_fresh_edit(at(1_000_000 - 3_600), now));
        assert!(!is_fresh_edit(None, now));
    }

    #[test]
    fn entries_fit_embed_limits() {
        let long = "ж".repeat(4_000);
        let origin = Origin {
            guild: GUILD,
            channel: CHANNEL,
            id: MessageId::new(1),
            author: UserId::new(3),
            author_name: "user",
        };
        let before = snapshot(1, &long);
        assert!(description_len(edited(&origin, Some(&before), &long, &[])) <= 4096);
        assert!(description_len(deleted(CHANNEL, MessageId::new(1), Some(&before), true)) <= 4096);
    }

    #[test]
    fn bulk_transcript_skips_automated_messages() {
        let bot = Snapshot {
            automated: true,
            ..snapshot(2, "")
        };
        let (_, file) = bulk_deleted(CHANNEL, 5, &[snapshot(1, "привет"), bot.clone()]);
        assert!(file.is_some());
        let (_, none) = bulk_deleted(CHANNEL, 5, &[bot]);
        assert!(none.is_none());
    }
}
