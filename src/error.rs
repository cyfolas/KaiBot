use std::error::Error;
use std::fmt;

use serenity::all::HttpError;

/// Ошибка обработки взаимодействия.
///
/// Варианты различаются тем, кому адресована ошибка: пользователю (он может её исправить)
/// или разработчикам (её нужно расследовать по журналу).
#[derive(Debug)]
pub enum AppError {
    /// Ожидаемая ситуация: невалидный ввод, нет прав, неподходящий контекст.
    /// Текст показывается пользователю как есть.
    User(String),
    /// Ошибка Discord API / Gateway. Пользователь видит обобщённое описание, детали уходят в журнал.
    Discord(Box<serenity::Error>),
}

impl AppError {
    pub fn user(message: impl Into<String>) -> Self {
        Self::User(message.into())
    }

    /// Взаимодействие, которое бот больше не умеет обрабатывать: команда удалена, кнопка или
    /// форма от старой версии бота, повреждённый `custom_id`.
    pub fn stale() -> Self {
        Self::user("Это действие устарело — вызовите команду заново.")
    }

    /// Код ошибки Discord JSON API, если это неуспешный HTTP-ответ.
    /// Коды: <https://discord.com/developers/docs/topics/opcodes-and-status-codes#json>
    pub fn discord_code(&self) -> Option<isize> {
        match self {
            Self::Discord(err) => match err.as_ref() {
                serenity::Error::Http(HttpError::UnsuccessfulRequest(response)) => {
                    Some(response.error.code)
                }
                _ => None,
            },
            Self::User(_) => None,
        }
    }

    /// Текст, который можно показать пользователю.
    pub fn user_message(&self) -> String {
        match self {
            Self::User(message) => message.clone(),
            Self::Discord(_) => match self.discord_code() {
                Some(50001) => "У бота нет доступа к этому каналу.".into(),
                Some(50013) => "У бота недостаточно прав для этого действия.".into(),
                Some(50035) => "Discord отклонил данные: проверьте ссылки и длину полей.".into(),
                _ => "Внутренняя ошибка. Подробности записаны в журнал бота.".into(),
            },
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(message) => f.write_str(message),
            // Сама ошибка Discord доступна через `source()`; см. `Chain`.
            Self::Discord(_) => f.write_str("ошибка Discord API"),
        }
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Discord(err) => Some(err.as_ref()),
            Self::User(_) => None,
        }
    }
}

impl From<serenity::Error> for AppError {
    fn from(err: serenity::Error) -> Self {
        Self::Discord(Box::new(err))
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

/// Ошибка, из-за которой бот не может работать: что не удалось и почему (причина — в `source()`).
#[derive(Debug)]
pub struct Fatal {
    what: String,
    cause: Option<Box<dyn Error + Send + Sync>>,
}

impl Fatal {
    pub fn new(what: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            cause: None,
        }
    }

    /// Для `map_err`: дополняет ошибку пояснением, что именно не удалось.
    pub fn context<E>(what: &'static str) -> impl FnOnce(E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        move |cause| Self {
            what: what.into(),
            cause: Some(Box::new(cause)),
        }
    }
}

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.what)
    }
}

impl Error for Fatal {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_deref().map(|cause| cause as _)
    }
}

/// Ошибка со всей цепочкой причин — для журнала.
///
/// `Display` у serenity скрывает исходную ошибку: например, вместо «unsupported scheme socks5h»
/// выводится только «Error while sending HTTP request.».
pub struct Chain<'a>(pub &'a (dyn Error + 'static));

impl fmt::Display for Chain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(cause) = source {
            write!(f, " → {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_prints_every_cause() {
        let io = std::io::Error::other("unsupported scheme socks5h");
        let fatal = Fatal::context("не удалось подключиться")(io);
        assert_eq!(
            Chain(&fatal).to_string(),
            "не удалось подключиться → unsupported scheme socks5h"
        );
    }
}
