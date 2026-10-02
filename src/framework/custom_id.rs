use std::fmt::Display;
use std::str::FromStr;

use crate::error::{AppError, Result};

/// Лимит Discord на длину `custom_id`.
const MAX_LEN: usize = 100;

/// `custom_id` кнопок и форм: `<команда>:<действие>[:<аргумент>…]`.
///
/// Первый сегмент — имя команды-владельца, по нему маршрутизирует `Handler`. Аргументы несут
/// состояние между шагами диалога, поэтому боту не нужно ничего хранить, а перезапуск не ломает
/// открытые формы. Аргументы приходят от клиента, поэтому каждый шаг проверяет их заново, как
/// и права.
pub struct CustomId<'a> {
    pub action: &'a str,
    args: std::str::Split<'a, char>,
}

impl<'a> CustomId<'a> {
    pub fn encode(command: &str, action: &str, args: &[&dyn Display]) -> String {
        let mut id = format!("{command}:{action}");
        for arg in args {
            id.push(':');
            id.push_str(&arg.to_string());
        }
        debug_assert!(id.len() <= MAX_LEN, "custom_id длиннее {MAX_LEN}: {id}");
        id
    }

    /// Имя команды-владельца.
    pub fn owner(raw: &str) -> &str {
        raw.split(':').next().unwrap_or_default()
    }

    pub fn parse(raw: &'a str) -> Self {
        let mut parts = raw.split(':');
        parts.next();
        let action = parts.next().unwrap_or_default();
        Self {
            action,
            args: parts,
        }
    }

    /// Следующий аргумент. Ошибка означает форму от старой версии бота или подделку.
    pub fn arg<T: FromStr>(&mut self) -> Result<T> {
        self.args
            .next()
            .and_then(|raw| raw.parse().ok())
            .ok_or_else(AppError::stale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let raw = CustomId::encode("embed", "modal", &[&42u64, &true]);
        assert_eq!(raw, "embed:modal:42:true");
        assert_eq!(CustomId::owner(&raw), "embed");

        let mut id = CustomId::parse(&raw);
        assert_eq!(id.action, "modal");
        assert_eq!(id.arg::<u64>().unwrap(), 42);
        assert!(id.arg::<bool>().unwrap());
        assert!(id.arg::<u64>().is_err());
    }

    #[test]
    fn malformed_ids_are_stale_not_panics() {
        assert_eq!(CustomId::owner(""), "");
        let mut id = CustomId::parse("say");
        assert_eq!(id.action, "");
        assert!(id.arg::<serenity::all::ChannelId>().is_err());

        let mut zero = CustomId::parse("say:modal:0");
        assert!(zero.arg::<serenity::all::ChannelId>().is_err());
    }
}
