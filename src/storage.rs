//! Хранение настроек в SQLite.
//!
//! Только перевод между типами настроек и строками таблиц; правила изменения — в
//! [`crate::settings`]. Схема — `migrations/`; миграции встроены в бинарник и применяются при
//! запуске, так что отдельная установка базы не нужна.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU64;
use std::path::Path;

use serenity::all::GuildId;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::access::{Effect, GlobalAccess, GlobalMode, GuildAccess, GuildMode, Subject};
use crate::error::Fatal;
use crate::settings::{GuildSettings, LogChannels};

static MIGRATOR: Migrator = sqlx::migrate!();

type Rows<T> = Result<T, sqlx::Error>;
/// Строка `guild_settings`: сервер, режим, каналы журналов сообщений, голоса и системного.
type GuildRow = (i64, String, Option<i64>, Option<i64>, Option<i64>);
/// Строка `access_rules`: сервер, вид субъекта, субъект, действие.
type RuleRow = (i64, String, i64, String);

/// Соединение с базой. Клонирование дешёвое: копируется ссылка на пул.
#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    /// Открывает (или создаёт) базу и применяет миграции.
    pub async fn open(path: &Path) -> Result<Self, Fatal> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // WAL: чтение не блокирует запись, а сбой посреди записи не портит базу.
            .journal_mode(SqliteJournalMode::Wal);
        Self::connect(options).await.map_err(Fatal::context(
            "не удалось открыть базу настроек (DATABASE_PATH)",
        ))
    }

    /// Пустая база в памяти — для тестов.
    #[cfg(test)]
    pub async fn in_memory() -> Rows<Self> {
        Self::connect("sqlite::memory:".parse()?).await
    }

    async fn connect(options: SqliteConnectOptions) -> Rows<Self> {
        // Одно соединение: записи и так идут по одной (см. `Settings`), а чтения — только при
        // запуске. Заодно база в памяти не теряется: она живёт, пока живо соединение.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options.foreign_keys(true))
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Все настройки: приложения и серверов.
    pub async fn load(&self) -> Rows<(GlobalAccess, HashMap<GuildId, GuildSettings>)> {
        let mode: String = sqlx::query_scalar("SELECT mode FROM global_settings WHERE id = 1")
            .fetch_one(&self.pool)
            .await?;
        let blocked: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM blocked_users")
            .fetch_all(&self.pool)
            .await?;
        let global = GlobalAccess {
            mode: parse_key(&mode, GlobalMode::from_key)?,
            blocked: blocked.into_iter().map(id).collect::<Rows<_>>()?,
        };

        let guild_rows: Vec<GuildRow> = sqlx::query_as(
            "SELECT guild_id, access_mode, message_log, voice_log, system_log \
                 FROM guild_settings",
        )
        .fetch_all(&self.pool)
        .await?;
        let rule_rows: Vec<RuleRow> =
            sqlx::query_as("SELECT guild_id, subject_kind, subject_id, effect FROM access_rules")
                .fetch_all(&self.pool)
                .await?;

        let mut rules: HashMap<GuildId, BTreeMap<Subject, Effect>> = HashMap::new();
        for (guild, kind, subject, effect) in rule_rows {
            let subject = match kind.as_str() {
                "user" => Subject::User(id(subject)?),
                "role" => Subject::Role(id(subject)?),
                other => return Err(corrupt(format!("вид субъекта «{other}»"))),
            };
            rules
                .entry(id(guild)?)
                .or_default()
                .insert(subject, parse_key(&effect, Effect::from_key)?);
        }

        let mut guilds = HashMap::with_capacity(guild_rows.len());
        for (guild, mode, messages, voice, system) in guild_rows {
            let guild: GuildId = id(guild)?;
            let access = GuildAccess::from_parts(
                parse_key(&mode, GuildMode::from_key)?,
                rules.remove(&guild).unwrap_or_default(),
            );
            let logs = LogChannels {
                messages: messages.map(id).transpose()?,
                voice: voice.map(id).transpose()?,
                system: system.map(id).transpose()?,
            };
            guilds.insert(guild, GuildSettings { access, logs });
        }
        Ok((global, guilds))
    }

    /// Сохраняет настройки сервера целиком в одной транзакции. Настройки по умолчанию
    /// не хранятся: строка сервера удаляется вместе с правилами.
    pub async fn save_guild(&self, guild: GuildId, settings: &GuildSettings) -> Rows<()> {
        let guild = sql_id(guild);
        let mut tx = self.pool.begin().await?;

        // Каскад удаляет и правила сервера.
        sqlx::query("DELETE FROM guild_settings WHERE guild_id = ?")
            .bind(guild)
            .execute(&mut *tx)
            .await?;

        if *settings != GuildSettings::default() {
            let logs = &settings.logs;
            sqlx::query(
                "INSERT INTO guild_settings \
                 (guild_id, access_mode, message_log, voice_log, system_log) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(guild)
            .bind(settings.access.mode.key())
            .bind(logs.messages.map(sql_id))
            .bind(logs.voice.map(sql_id))
            .bind(logs.system.map(sql_id))
            .execute(&mut *tx)
            .await?;

            for (subject, effect) in settings.access.rules() {
                let (kind, subject) = match *subject {
                    Subject::User(user) => ("user", sql_id(user)),
                    Subject::Role(role) => ("role", sql_id(role)),
                };
                sqlx::query(
                    "INSERT INTO access_rules (guild_id, subject_kind, subject_id, effect) \
                     VALUES (?, ?, ?, ?)",
                )
                .bind(guild)
                .bind(kind)
                .bind(subject)
                .bind(effect.key())
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await
    }

    /// Сохраняет настройки приложения целиком в одной транзакции.
    pub async fn save_global(&self, global: &GlobalAccess) -> Rows<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE global_settings SET mode = ? WHERE id = 1")
            .bind(global.mode.key())
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM blocked_users")
            .execute(&mut *tx)
            .await?;
        for &user in &global.blocked {
            sqlx::query("INSERT INTO blocked_users (user_id) VALUES (?)")
                .bind(sql_id(user))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }
}

/// Snowflake → INTEGER с тем же битовым представлением: преобразование взаимно однозначное.
fn sql_id(id: impl Into<NonZeroU64>) -> i64 {
    id.into().get().cast_signed()
}

/// INTEGER → ID Discord. Ноль запрещён схемой, но база — внешние данные, поэтому проверяется.
fn id<T: From<NonZeroU64>>(raw: i64) -> Rows<T> {
    NonZeroU64::new(raw.cast_unsigned())
        .map(T::from)
        .ok_or_else(|| corrupt("нулевой ID".into()))
}

fn parse_key<T>(key: &str, parse: impl Fn(&str) -> Option<T>) -> Rows<T> {
    parse(key).ok_or_else(|| corrupt(format!("неизвестное значение «{key}»")))
}

fn corrupt(what: String) -> sqlx::Error {
    sqlx::Error::Decode(format!("повреждённые настройки в базе: {what}").into())
}

#[cfg(test)]
mod tests {
    use serenity::all::{ChannelId, RoleId, UserId};

    use super::*;

    #[test]
    fn ids_round_trip_through_signed_integers() {
        for raw in [1, 1_234_567_890_123_456_789, u64::MAX] {
            let user = UserId::new(raw);
            assert_eq!(id::<UserId>(sql_id(user)).unwrap(), user);
        }
        assert!(id::<GuildId>(0).is_err());
    }

    #[tokio::test]
    async fn fresh_database_has_defaults() {
        let storage = Storage::in_memory().await.unwrap();
        let (global, guilds) = storage.load().await.unwrap();
        assert_eq!(global, GlobalAccess::default());
        assert!(guilds.is_empty());
    }

    #[tokio::test]
    async fn default_settings_are_not_stored() {
        let storage = Storage::in_memory().await.unwrap();
        let guild = GuildId::new(1);
        let mut settings = GuildSettings::default();
        settings.access.mode = GuildMode::Locked;
        settings
            .access
            .set_rule(Subject::Role(RoleId::new(2)), Effect::Deny)
            .unwrap();
        settings.logs.voice = Some(ChannelId::new(3));
        storage.save_guild(guild, &settings).await.unwrap();
        assert_eq!(storage.load().await.unwrap().1[&guild], settings);

        storage
            .save_guild(guild, &GuildSettings::default())
            .await
            .unwrap();
        assert!(storage.load().await.unwrap().1.is_empty());
        let rules: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM access_rules")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!(rules, 0, "правила удаляются каскадом");
    }
}
