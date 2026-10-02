//! Журналы сервера в Discord: сообщения, голосовые каналы и системный журнал.
//!
//! Каналы журналов выбирает администрация (`/logs`); если журнал выключен, бот не собирает для
//! него никаких данных. Запись в журнал — побочное действие: её ошибка попадает в журнал
//! процесса и никогда не прерывает основное действие. Все записи отправляются без уведомлений:
//! упоминания в них только отображаются.
//!
//! Отдельно бот следит за положением своей роли: после изменения ролей сервера (с задержкой,
//! чтобы пачка событий перестановки ролей дала одну проверку) он сравнивает, какими
//! административными ролями может управлять, и сообщает в системный журнал, если это изменилось.

mod messages;
pub mod system;
mod voice;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serenity::all::*;
use tracing::warn;

pub use self::messages::PER_CHANNEL as MESSAGES_PER_CHANNEL;
use self::messages::{Edit, MessageStore, Origin, Snapshot};
use crate::error::{AppError, Chain};
use crate::framework::Cx;
use crate::hierarchy::{Hierarchy, Person, Standing};
use crate::settings::LogKind;
use crate::state::AppState;

/// Коды ошибок Discord: <https://discord.com/developers/docs/topics/opcodes-and-status-codes#json>
const UNKNOWN_CHANNEL: isize = 10003;
const MISSING_ACCESS: isize = 50001;
const MISSING_PERMISSIONS: isize = 50013;

/// Сообщение о недоступном журнале — не чаще раза в этот интервал на журнал.
const NOTICE_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Пауза перед проверкой положения роли бота: собирает пачку событий в одну проверку.
const STANDING_DEBOUNCE: Duration = Duration::from_secs(3);

/// Состояние журналов в памяти процесса.
#[derive(Default)]
pub struct Journal {
    messages: MessageStore,
    /// Когда последний раз сообщали о недоступности журнала.
    notices: Mutex<HashMap<(GuildId, LogKind), Instant>>,
    /// Последнее известное положение роли бота на серверах.
    standings: Mutex<HashMap<GuildId, Standing>>,
    /// Серверы, для которых проверка положения уже запланирована.
    pending: Mutex<HashSet<GuildId>>,
}

impl Journal {
    /// Забыть запомненные сообщения сервера — при выключении журнала сообщений.
    pub fn forget_messages(&self, guild: GuildId) {
        self.messages.forget_guild(guild);
    }

    /// Забыть запомненные сообщения канала — когда он становится каналом журнала.
    pub fn forget_channel(&self, channel: ChannelId) {
        self.messages.forget_channel(channel);
    }

    fn should_notify(&self, guild: GuildId, kind: LogKind) -> bool {
        let now = Instant::now();
        let mut notices = lock(&self.notices);
        match notices.get(&(guild, kind)) {
            Some(&at) if now.duration_since(at) < NOTICE_INTERVAL => false,
            _ => {
                notices.insert((guild, kind), now);
                true
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Запись в журнал `kind` сервера; если журнал выключен, ничего не делает.
pub async fn post(
    http: &Http,
    state: &AppState,
    guild: GuildId,
    kind: LogKind,
    message: CreateMessage,
) {
    let Some(channel) = state.settings.guild(guild).logs.get(kind) else {
        return;
    };
    let Err(err) = send(http, channel, message).await else {
        return;
    };

    warn!(
        "Журнал «{}» сервера {guild} (канал {channel}): {}",
        kind.key(),
        Chain(&err)
    );
    match err.discord_code() {
        // Канал удалён, пока бот был отключён: событие удаления не пришло.
        Some(UNKNOWN_CHANNEL) => forget_log_channel(http, state, guild, channel).await,
        Some(MISSING_ACCESS | MISSING_PERMISSIONS)
            if kind != LogKind::System && state.journal.should_notify(guild, kind) =>
        {
            let notice = system::log_unavailable(kind, channel, &err.user_message());
            post_system_only(http, state, guild, notice).await;
        }
        _ => {}
    }
}

/// Запись в системный журнал от имени команды.
pub async fn system(cx: Cx<'_>, entry: CreateEmbed) {
    post(
        &cx.ctx.http,
        cx.state,
        cx.caller.guild_id,
        LogKind::System,
        CreateMessage::new().embed(entry),
    )
    .await;
}

/// Запись в системный журнал без обработки его собственных ошибок — для сообщений о неполадках
/// других журналов, чтобы ошибка системного журнала не порождала новых записей.
async fn post_system_only(http: &Http, state: &AppState, guild: GuildId, entry: CreateEmbed) {
    let Some(channel) = state.settings.guild(guild).logs.system else {
        return;
    };
    if let Err(err) = send(http, channel, CreateMessage::new().embed(entry)).await {
        warn!("Системный журнал сервера {guild}: {}", Chain(&err));
    }
}

async fn send(http: &Http, channel: ChannelId, message: CreateMessage) -> Result<(), AppError> {
    channel
        .send_message(http, message.allowed_mentions(CreateAllowedMentions::new()))
        .await?;
    Ok(())
}

/// Выключает журналы, которые вели в удалённом канале, и сообщает об этом.
async fn forget_log_channel(http: &Http, state: &AppState, guild: GuildId, channel: ChannelId) {
    let logs = state.settings.guild(guild).logs;
    let kinds: Vec<LogKind> = LogKind::ALL
        .into_iter()
        .filter(|&kind| logs.get(kind) == Some(channel))
        .collect();
    if kinds.is_empty() {
        return;
    }

    let outcome = state
        .settings
        .update_guild(guild, |settings| {
            for &kind in &kinds {
                settings.logs.set(kind, None);
            }
            Ok(())
        })
        .await;
    match outcome {
        Ok(()) => {
            if kinds.contains(&LogKind::Messages) {
                state.journal.forget_messages(guild);
            }
            post_system_only(
                http,
                state,
                guild,
                system::log_channel_deleted(&kinds, channel),
            )
            .await;
        }
        Err(err) => warn!(
            "Не удалось выключить журналы сервера {guild}: {}",
            Chain(&err)
        ),
    }
}

/// Виден ли канал участнику без ролей: права @everyone с её оверрайтом канала — так считает
/// Discord.
pub fn is_public(guild: &Guild, channel: &GuildChannel) -> bool {
    let everyone = RoleId::new(guild.id.get());
    let base = guild
        .roles
        .get(&everyone)
        .map_or(Permissions::empty(), |role| role.permissions);
    let overwrite = channel
        .permission_overwrites
        .iter()
        .find(|overwrite| overwrite.kind == PermissionOverwriteType::Role(everyone))
        .map(|overwrite| (overwrite.allow, overwrite.deny));
    visible_to_everyone(base, overwrite)
}

fn visible_to_everyone(base: Permissions, overwrite: Option<(Permissions, Permissions)>) -> bool {
    if base.administrator() {
        return true;
    }
    let granted = match overwrite {
        Some((allow, deny)) => (base - deny) | allow,
        None => base,
    };
    granted.view_channel()
}

/// Текущее положение роли бота: роли — из кэша, роли самого бота — из Discord (без Server
/// Members Intent кэш о них не узнаёт).
pub async fn standing(ctx: &Context, guild: GuildId) -> Result<Standing, AppError> {
    let bot = ctx.cache.current_user().id;
    let member = ctx.http.get_member(guild, bot).await?;
    let guild = ctx.cache.guild(guild).ok_or_else(|| {
        AppError::user("Данные сервера ещё загружаются, повторите через несколько секунд.")
    })?;
    Ok(Hierarchy::of(&guild).standing(Person {
        id: bot,
        roles: &member.roles,
        bot: true,
    }))
}

/// Обработчик событий Gateway для журналов.
pub struct Watcher {
    state: Arc<AppState>,
}

impl Watcher {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Журнал сообщений включён и канал не является каналом журнала (иначе бот записывал бы
    /// собственные записи).
    fn logs_messages_in(&self, guild: GuildId, channel: ChannelId) -> bool {
        let logs = self.state.settings.guild(guild).logs;
        logs.messages.is_some() && !logs.contains(channel)
    }

    async fn post(&self, ctx: &Context, guild: GuildId, kind: LogKind, message: CreateMessage) {
        if kind == LogKind::Messages && self.message_log_is_public(ctx, guild) {
            return;
        }
        post(&ctx.http, &self.state, guild, kind, message).await;
    }

    /// Журнал сообщений приостанавливается, если его канал стал виден всем после выбора
    /// (`/logs set` такой канал не примет): тексты из закрытых каналов не должны стать публичными.
    fn message_log_is_public(&self, ctx: &Context, guild: GuildId) -> bool {
        let Some(channel) = self.state.settings.guild(guild).logs.messages else {
            return false;
        };
        let public = ctx.cache.guild(guild).is_some_and(|cached| {
            cached
                .channels
                .get(&channel)
                .is_some_and(|log| is_public(&cached, log))
        });
        if public && self.state.journal.should_notify(guild, LogKind::Messages) {
            let notice = system::log_unavailable(
                LogKind::Messages,
                channel,
                "канал стал виден всем (@everyone), журнал приостановлен.",
            );
            let (http, state) = (Arc::clone(&ctx.http), Arc::clone(&self.state));
            tokio::spawn(async move { post_system_only(&http, &state, guild, notice).await });
        }
        public
    }

    /// Планирует проверку положения роли бота (см. описание модуля).
    fn schedule_standing_check(&self, ctx: &Context, guild: GuildId) {
        if !lock(&self.state.journal.pending).insert(guild) {
            return;
        }
        let (ctx, state) = (ctx.clone(), Arc::clone(&self.state));
        tokio::spawn(async move {
            tokio::time::sleep(STANDING_DEBOUNCE).await;
            // Снимается до проверки: события во время проверки запланируют следующую.
            lock(&state.journal.pending).remove(&guild);

            let current = match standing(&ctx, guild).await {
                Ok(current) => current,
                Err(err) => {
                    warn!("Положение роли бота на сервере {guild}: {}", Chain(&err));
                    return;
                }
            };
            let previous = lock(&state.journal.standings).insert(guild, current.clone());
            if previous.is_some_and(|previous| !previous.same_reach(&current)) {
                let entry = system::standing_changed(&current);
                post(
                    &ctx.http,
                    &state,
                    guild,
                    LogKind::System,
                    CreateMessage::new().embed(entry),
                )
                .await;
            }
        });
    }
}

#[async_trait]
impl EventHandler for Watcher {
    /// Исходное положение роли бота запоминается молча: сообщается только об изменениях.
    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: Option<bool>) {
        let bot = ctx.cache.current_user().id;
        match guild.members.get(&bot) {
            Some(member) => {
                let standing = Hierarchy::of(&guild).standing(Person {
                    id: bot,
                    roles: &member.roles,
                    bot: true,
                });
                lock(&self.state.journal.standings).insert(guild.id, standing);
            }
            None => self.schedule_standing_check(&ctx, guild.id),
        }
    }

    /// Бота удалили с сервера: забыть всё, что относится к нему в памяти. Настройки остаются —
    /// при повторном приглашении их не придётся задавать заново.
    async fn guild_delete(
        &self,
        _ctx: Context,
        incomplete: UnavailableGuild,
        _full: Option<Guild>,
    ) {
        if !incomplete.unavailable {
            self.state.journal.forget_messages(incomplete.id);
            lock(&self.state.journal.standings).remove(&incomplete.id);
        }
    }

    async fn channel_delete(
        &self,
        ctx: Context,
        channel: GuildChannel,
        _messages: Option<Vec<Message>>,
    ) {
        self.state.journal.forget_channel(channel.id);
        forget_log_channel(&ctx.http, &self.state, channel.guild_id, channel.id).await;
    }

    async fn message(&self, _ctx: Context, message: Message) {
        let Some(guild) = message.guild_id else {
            return;
        };
        // Только сообщения участников; системные (закрепление, бусты, приветствия) не нужны.
        if !matches!(
            message.kind,
            MessageType::Regular | MessageType::InlineReply
        ) || !self.logs_messages_in(guild, message.channel_id)
        {
            return;
        }
        self.state
            .journal
            .messages
            .remember(guild, message.channel_id, Snapshot::of(&message));
    }

    async fn message_update(
        &self,
        ctx: Context,
        _old: Option<Message>,
        _new: Option<Message>,
        event: MessageUpdateEvent,
    ) {
        let Some(guild) = event.guild_id else {
            return;
        };
        let Some(content) = event.content.as_deref() else {
            return;
        };
        if !self.logs_messages_in(guild, event.channel_id) {
            return;
        }
        let attachments = event.attachments.as_deref().map(messages::attachment_names);
        let store = &self.state.journal.messages;

        let entry = match store.edit(event.channel_id, event.id, content, attachments.clone()) {
            Edit::Unchanged => return,
            Edit::Changed(before) => {
                let origin = Origin {
                    guild,
                    channel: event.channel_id,
                    id: event.id,
                    author: before.author,
                    author_name: &before.author_name,
                };
                messages::edited(
                    &origin,
                    Some(&before),
                    content,
                    &store_attachments(&before, attachments),
                )
            }
            Edit::Unknown => {
                let Some(author) = &event.author else {
                    return;
                };
                let automated = author.bot || matches!(event.webhook_id, Some(Some(_)));
                let attachments = attachments.unwrap_or_default();
                store.remember(
                    guild,
                    event.channel_id,
                    Snapshot {
                        id: event.id,
                        author: author.id,
                        author_name: author.name.clone(),
                        automated,
                        content: if automated {
                            String::new()
                        } else {
                            content.to_string()
                        },
                        attachments: if automated {
                            Vec::new()
                        } else {
                            attachments.clone()
                        },
                    },
                );
                if automated || !messages::is_fresh_edit(event.edited_timestamp, Timestamp::now()) {
                    return;
                }
                let origin = Origin {
                    guild,
                    channel: event.channel_id,
                    id: event.id,
                    author: author.id,
                    author_name: &author.name,
                };
                messages::edited(&origin, None, content, &attachments)
            }
        };
        self.post(
            &ctx,
            guild,
            LogKind::Messages,
            CreateMessage::new().embed(entry),
        )
        .await;
    }

    async fn message_delete(
        &self,
        ctx: Context,
        channel: ChannelId,
        id: MessageId,
        guild: Option<GuildId>,
    ) {
        let Some(guild) = guild else {
            return;
        };
        if !self.logs_messages_in(guild, channel) {
            return;
        }
        let snapshot = self.state.journal.messages.take(channel, id);
        if snapshot.as_ref().is_some_and(|s| s.automated) {
            return;
        }
        let content_intent = self
            .state
            .intents()
            .contains(GatewayIntents::MESSAGE_CONTENT);
        let entry = messages::deleted(channel, id, snapshot.as_ref(), content_intent);
        self.post(
            &ctx,
            guild,
            LogKind::Messages,
            CreateMessage::new().embed(entry),
        )
        .await;
    }

    async fn message_delete_bulk(
        &self,
        ctx: Context,
        channel: ChannelId,
        ids: Vec<MessageId>,
        guild: Option<GuildId>,
    ) {
        let Some(guild) = guild else {
            return;
        };
        if !self.logs_messages_in(guild, channel) {
            return;
        }
        let known = self.state.journal.messages.take_many(channel, &ids);
        let (entry, transcript) = messages::bulk_deleted(channel, ids.len(), &known);
        let mut message = CreateMessage::new().embed(entry);
        if let Some(file) = transcript {
            message = message.add_file(file);
        }
        self.post(&ctx, guild, LogKind::Messages, message).await;
    }

    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        let Some(guild) = new.guild_id else {
            return;
        };
        if self.state.settings.guild(guild).logs.voice.is_none() {
            return;
        }
        let Some(transition) =
            voice::transition(old.and_then(|state| state.channel_id), new.channel_id)
        else {
            return;
        };
        let name = match &new.member {
            Some(member) => member.display_name().to_string(),
            None => new.user_id.to_string(),
        };
        let entry = voice::entry(new.user_id, &name, transition);
        self.post(
            &ctx,
            guild,
            LogKind::Voice,
            CreateMessage::new().embed(entry),
        )
        .await;
    }

    async fn guild_role_create(&self, ctx: Context, role: Role) {
        self.schedule_standing_check(&ctx, role.guild_id);
    }

    async fn guild_role_update(&self, ctx: Context, _old: Option<Role>, role: Role) {
        self.schedule_standing_check(&ctx, role.guild_id);
    }

    async fn guild_role_delete(
        &self,
        ctx: Context,
        guild: GuildId,
        _role: RoleId,
        _data: Option<Role>,
    ) {
        self.schedule_standing_check(&ctx, guild);
    }

    /// Приходит только с Server Members Intent; без него изменения ролей самого бота
    /// заметны при следующем изменении ролей сервера.
    async fn guild_member_update(
        &self,
        ctx: Context,
        _old: Option<Member>,
        _new: Option<Member>,
        event: GuildMemberUpdateEvent,
    ) {
        if event.user.id == ctx.cache.current_user().id {
            self.schedule_standing_check(&ctx, event.guild_id);
        }
    }
}

/// Вложения после изменения: если Discord их не прислал, они не менялись.
fn store_attachments(before: &Snapshot, attachments: Option<Vec<String>>) -> Vec<String> {
    attachments.unwrap_or_else(|| before.attachments.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everyone_visibility_follows_discord_overwrites() {
        let view = Permissions::VIEW_CHANNEL;
        assert!(visible_to_everyone(view, None));
        assert!(!visible_to_everyone(
            view,
            Some((Permissions::empty(), view))
        ));
        assert!(!visible_to_everyone(Permissions::empty(), None));
        assert!(visible_to_everyone(
            Permissions::empty(),
            Some((view, Permissions::empty()))
        ));
        // Администратор у @everyone видит всё, оверрайты не действуют.
        assert!(visible_to_everyone(
            Permissions::ADMINISTRATOR,
            Some((Permissions::empty(), view))
        ));
    }
}
