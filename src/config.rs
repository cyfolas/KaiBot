use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;

use serenity::all::UserId;

use crate::error::Fatal;

const DEFAULT_DATABASE: &str = "axiom.db";

/// Конфигурация процесса из переменных окружения (`.env`).
///
/// Здесь только то, чего нельзя узнать у Discord: токен, разработчики бота и место хранения
/// настроек. Всё, что относится к конкретному серверу (владелец, роли, их иерархия и права),
/// бот читает из Discord в момент обращения — см. `framework::Caller`, а решения администрации
/// (доступ, журналы) хранит в базе — см. `settings`.
pub struct Config {
    pub token: String,
    /// Явно указанные разработчики (`BOT_DEVELOPERS`). При запуске дополняются командой или
    /// владельцем приложения из Developer Portal.
    pub developers: BTreeSet<UserId>,
    /// Файл базы настроек (`DATABASE_PATH`, по умолчанию `axiom.db`).
    pub database: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self, Fatal> {
        let token = env::var("DISCORD_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| Fatal::new("не задана переменная окружения DISCORD_TOKEN"))?;

        let developers = match env::var("BOT_DEVELOPERS") {
            Ok(raw) => parse_user_ids(&raw).map_err(|bad| {
                Fatal::new(format!(
                    "BOT_DEVELOPERS: «{bad}» не является ID пользователя"
                ))
            })?,
            Err(_) => BTreeSet::new(),
        };

        let database = env::var_os("DATABASE_PATH")
            .filter(|path| !path.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_DATABASE), PathBuf::from);

        Ok(Self {
            token,
            developers,
            database,
        })
    }
}

/// Разбирает ID, разделённые запятыми и/или пробелами. Ошибка — первый невалидный элемент.
fn parse_user_ids(raw: &str) -> Result<BTreeSet<UserId>, &str> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(|part| part.trim_matches('"'))
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<UserId>().map_err(|_| part))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comma_and_space_separated_ids() {
        let ids = parse_user_ids(r#""966317412733562901, 501085375251480586""#).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&UserId::new(501_085_375_251_480_586)));
    }

    #[test]
    fn empty_list_is_valid() {
        assert!(parse_user_ids("  ").unwrap().is_empty());
    }

    #[test]
    fn rejects_garbage_and_zero() {
        assert_eq!(parse_user_ids("123,abc"), Err("abc"));
        assert_eq!(parse_user_ids("0"), Err("0"));
        assert_eq!(parse_user_ids("-5"), Err("-5"));
    }
}
