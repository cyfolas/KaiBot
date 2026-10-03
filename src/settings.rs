//! Настройки бота: уровень приложения (разработчики) и уровень серверов (администрация).
//!
//! Источник истины — SQLite ([`Storage`]), а читаются настройки из памяти: они нужны на каждом
//! взаимодействии и на каждом событии журнала. Изменение проходит по одному пути — копия,
//! изменение, запись в базу и только после успешной записи публикация в памяти. Поэтому память
//! никогда не опережает базу, а неудачная запись ничего не меняет.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use serenity::all::{ChannelId, GuildId};
use tokio::sync::Mutex;

use crate::domain::access::{GlobalAccess, GuildAccess};
use crate::domain::policy::Policy;
use crate::error::{AppError, Fatal, Result};
use crate::storage::Storage;

/// Настройки сервера. Значение по умолчанию — для сервера, который бота ещё не настраивал.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildSettings {
    pub access: GuildAccess,
    pub logs: LogChannels,
    /// Политика прав; `None` — не настроена (нет роли участника).
    pub policy: Option<Policy>,
}

/// Вид журнала сервера.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogKind {
    /// Изменённые и удалённые сообщения.
    Messages,
    /// Входы, выходы и переходы в голосовых каналах.
    Voice,
    /// Важные события бота: настройки, доступ, администрация, публикации от имени бота.
    System,
}

impl LogKind {
    pub const ALL: [Self; 3] = [Self::Messages, Self::Voice, Self::System];

    /// Значение в опциях команд.
    pub fn key(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Voice => "voice",
            Self::System => "system",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Messages => "Сообщения",
            Self::Voice => "Голосовые каналы",
            Self::System => "Системный",
        }
    }
}

/// Каналы журналов сервера; `None` — журнал выключен.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogChannels {
    pub messages: Option<ChannelId>,
    pub voice: Option<ChannelId>,
    pub system: Option<ChannelId>,
}

impl LogChannels {
    pub fn get(&self, kind: LogKind) -> Option<ChannelId> {
        *self.slot(kind)
    }

    /// Устанавливает канал журнала и возвращает прежний.
    pub fn set(&mut self, kind: LogKind, channel: Option<ChannelId>) -> Option<ChannelId> {
        std::mem::replace(self.slot_mut(kind), channel)
    }

    /// Является ли канал каналом какого-либо журнала.
    pub fn contains(&self, channel: ChannelId) -> bool {
        LogKind::ALL
            .into_iter()
            .any(|kind| self.get(kind) == Some(channel))
    }

    fn slot(&self, kind: LogKind) -> &Option<ChannelId> {
        match kind {
            LogKind::Messages => &self.messages,
            LogKind::Voice => &self.voice,
            LogKind::System => &self.system,
        }
    }

    fn slot_mut(&mut self, kind: LogKind) -> &mut Option<ChannelId> {
        match kind {
            LogKind::Messages => &mut self.messages,
            LogKind::Voice => &mut self.voice,
            LogKind::System => &mut self.system,
        }
    }
}

/// Настройки в памяти поверх [`Storage`].
pub struct Settings {
    storage: Storage,
    global: RwLock<Arc<GlobalAccess>>,
    guilds: RwLock<HashMap<GuildId, Arc<GuildSettings>>>,
    /// Общий для серверов, у которых нет своих настроек.
    default_guild: Arc<GuildSettings>,
    /// Изменения выполняются строго по одному: иначе два одновременных изменения прочитали бы
    /// одну версию и второе затёрло бы первое. Изменения редки, так что общая очередь не мешает.
    writes: Mutex<()>,
}

impl Settings {
    pub async fn load(storage: Storage) -> std::result::Result<Self, Fatal> {
        let (global, guilds) = storage
            .load()
            .await
            .map_err(Fatal::context("не удалось прочитать настройки из базы"))?;

        Ok(Self {
            storage,
            global: RwLock::new(Arc::new(global)),
            guilds: RwLock::new(
                guilds
                    .into_iter()
                    .map(|(id, settings)| (id, Arc::new(settings)))
                    .collect(),
            ),
            default_guild: Arc::default(),
            writes: Mutex::new(()),
        })
    }

    pub fn global(&self) -> Arc<GlobalAccess> {
        Arc::clone(&self.global.read().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn guild(&self, guild: GuildId) -> Arc<GuildSettings> {
        let guilds = self.guilds.read().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(guilds.get(&guild).unwrap_or(&self.default_guild))
    }

    /// Изменяет настройки сервера. `change` может отказать — тогда ничего не меняется.
    /// Если настройки не изменились, запись в базу не выполняется.
    pub async fn update_guild<R>(
        &self,
        guild: GuildId,
        change: impl FnOnce(&mut GuildSettings) -> Result<R>,
    ) -> Result<R> {
        let _queue = self.writes.lock().await;
        let current = self.guild(guild);
        let mut next = GuildSettings::clone(&current);
        let outcome = change(&mut next)?;

        if next != *current {
            self.storage
                .save_guild(guild, &next)
                .await
                .map_err(AppError::from)?;
            let mut guilds = self.guilds.write().unwrap_or_else(PoisonError::into_inner);
            if next == *self.default_guild {
                guilds.remove(&guild);
            } else {
                guilds.insert(guild, Arc::new(next));
            }
        }
        Ok(outcome)
    }

    /// Изменяет настройки приложения — по тем же правилам, что и [`Self::update_guild`].
    pub async fn update_global<R>(
        &self,
        change: impl FnOnce(&mut GlobalAccess) -> Result<R>,
    ) -> Result<R> {
        let _queue = self.writes.lock().await;
        let current = self.global();
        let mut next = GlobalAccess::clone(&current);
        let outcome = change(&mut next)?;

        if next != *current {
            self.storage
                .save_global(&next)
                .await
                .map_err(AppError::from)?;
            *self.global.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
        }
        Ok(outcome)
    }

    /// Число серверов с собственными настройками.
    pub fn configured_guilds(&self) -> usize {
        self.guilds
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use serenity::all::UserId;

    use super::*;
    use crate::domain::access::{Effect, GlobalMode, GuildMode, Subject};

    async fn settings() -> Settings {
        Settings::load(Storage::in_memory().await.unwrap())
            .await
            .unwrap()
    }

    #[test]
    fn log_channels_slots_are_independent() {
        let mut logs = LogChannels::default();
        let channel = ChannelId::new(5);
        assert_eq!(logs.set(LogKind::Voice, Some(channel)), None);
        assert_eq!(logs.get(LogKind::Voice), Some(channel));
        assert_eq!(logs.get(LogKind::Messages), None);
        assert!(logs.contains(channel));
        assert_eq!(logs.set(LogKind::Voice, None), Some(channel));
        assert!(!logs.contains(channel));
    }

    #[test]
    fn log_kind_keys_round_trip() {
        for kind in LogKind::ALL {
            assert_eq!(LogKind::from_key(kind.key()), Some(kind));
        }
    }

    #[tokio::test]
    async fn changes_survive_reload() {
        let storage = Storage::in_memory().await.unwrap();
        let settings = Settings::load(storage.clone()).await.unwrap();
        let guild = GuildId::new(1);

        settings
            .update_guild(guild, |s| {
                s.access.mode = GuildMode::Restricted;
                s.access
                    .set_rule(Subject::User(UserId::new(7)), Effect::Allow)
                    .unwrap();
                s.logs.set(LogKind::System, Some(ChannelId::new(9)));
                Ok(())
            })
            .await
            .unwrap();
        settings
            .update_global(|g| {
                g.mode = GlobalMode::DevOnly;
                g.blocked.insert(UserId::new(3));
                Ok(())
            })
            .await
            .unwrap();

        let reloaded = Settings::load(storage).await.unwrap();
        assert_eq!(*reloaded.guild(guild), *settings.guild(guild));
        assert_eq!(*reloaded.global(), *settings.global());
        assert_eq!(reloaded.guild(guild).access.mode, GuildMode::Restricted);
    }

    #[tokio::test]
    async fn refused_change_leaves_settings_untouched() {
        let settings = settings().await;
        let guild = GuildId::new(1);
        let outcome = settings
            .update_guild(guild, |s| {
                s.access.mode = GuildMode::Locked;
                Err::<(), _>(AppError::user("нельзя"))
            })
            .await;
        assert!(outcome.is_err());
        assert_eq!(settings.guild(guild).access.mode, GuildMode::Open);
    }

    #[tokio::test]
    async fn resetting_to_defaults_forgets_the_guild() {
        let settings = settings().await;
        let guild = GuildId::new(1);
        settings
            .update_guild(guild, |s| {
                s.access.mode = GuildMode::Locked;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(settings.configured_guilds(), 1);

        settings
            .update_guild(guild, |s| {
                s.access.mode = GuildMode::Open;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(settings.configured_guilds(), 0);
    }
}
