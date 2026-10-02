//! Сборка и валидация embed. Чистая логика без обращений к Discord — покрыта тестами.

use serenity::all::{Colour, CreateEmbed, CreateEmbedFooter, Timestamp};

use crate::error::{AppError, Result};

// Лимиты Discord: https://discord.com/developers/docs/resources/message#embed-object-embed-limits
pub const TITLE_MAX: u16 = 256;
pub const FOOTER_MAX: u16 = 2048;
const DESCRIPTION_MAX: usize = 4096;
const TOTAL_MAX: usize = 6000;

/// Поля будущего embed. Пустые строки и строки из пробелов считаются незаполненными.
#[derive(Default)]
pub struct EmbedDraft<'a> {
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub image_url: Option<&'a str>,
    pub thumbnail_url: Option<&'a str>,
    pub footer: Option<&'a str>,
    pub color: Colour,
    pub timestamp: bool,
}

impl EmbedDraft<'_> {
    pub fn build(&self) -> Result<CreateEmbed> {
        let title = present(self.title);
        let description = present(self.description);
        let image_url = present(self.image_url);
        let thumbnail_url = present(self.thumbnail_url);
        let footer = present(self.footer);

        if title.is_none() && description.is_none() && image_url.is_none() {
            return Err(AppError::user(
                "Заполните хотя бы заголовок, текст или ссылку на изображение.",
            ));
        }

        check_len("Заголовок", title, TITLE_MAX.into())?;
        check_len("Текст", description, DESCRIPTION_MAX)?;
        check_len("Подпись", footer, FOOTER_MAX.into())?;
        let total: usize = [title, description, footer]
            .into_iter()
            .flatten()
            .map(|s| s.chars().count())
            .sum();
        if total > TOTAL_MAX {
            return Err(AppError::user(format!(
                "Суммарный объём текста embed не может превышать {TOTAL_MAX} символов."
            )));
        }

        let mut embed = CreateEmbed::new().colour(self.color);
        if let Some(title) = title {
            embed = embed.title(title);
        }
        if let Some(description) = description {
            embed = embed.description(description);
        }
        if let Some(url) = image_url {
            check_url("Изображение", url)?;
            embed = embed.image(url);
        }
        if let Some(url) = thumbnail_url {
            check_url("Миниатюра", url)?;
            embed = embed.thumbnail(url);
        }
        if let Some(footer) = footer {
            embed = embed.footer(CreateEmbedFooter::new(footer));
        }
        if self.timestamp {
            embed = embed.timestamp(Timestamp::now());
        }
        Ok(embed)
    }
}

fn present(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn check_len(field: &str, value: Option<&str>, max: usize) -> Result<()> {
    match value {
        Some(text) if text.chars().count() > max => {
            Err(AppError::user(format!("{field}: не более {max} символов.")))
        }
        _ => Ok(()),
    }
}

/// Discord принимает в embed только абсолютные ссылки http(s).
fn check_url(field: &str, url: &str) -> Result<()> {
    let host_and_path = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    let valid = matches!(host_and_path, Some(rest) if !rest.is_empty() && !rest.starts_with('/'))
        && !url.chars().any(char::is_whitespace);

    if valid {
        Ok(())
    } else {
        Err(AppError::user(format!(
            "{field}: нужна ссылка вида https://example.com/image.png"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::embed::color;

    #[test]
    fn validates_urls() {
        assert!(check_url("x", "https://cdn.discordapp.com/image.png").is_ok());
        assert!(check_url("x", "http://example.com/pic.jpg").is_ok());
        assert!(check_url("x", "ftp://example.com/pic.jpg").is_err());
        assert!(check_url("x", "https://").is_err());
        assert!(check_url("x", "https:///path").is_err());
        assert!(check_url("x", "https://exa mple.com").is_err());
    }

    #[test]
    fn requires_visible_content() {
        let blank = EmbedDraft {
            title: Some("   "),
            footer: Some("подпись"),
            ..Default::default()
        };
        assert!(blank.build().is_err());

        let image_only = EmbedDraft {
            image_url: Some("https://example.com/banner.png"),
            ..Default::default()
        };
        assert!(image_only.build().is_ok());
    }

    #[test]
    fn enforces_limits() {
        let long_title = "a".repeat(usize::from(TITLE_MAX) + 1);
        let draft = EmbedDraft {
            title: Some(&long_title),
            ..Default::default()
        };
        assert!(draft.build().is_err());

        // Каждое поле в пределах лимита, но вместе больше 6000.
        let description = "b".repeat(DESCRIPTION_MAX);
        let footer = "c".repeat(FOOTER_MAX.into());
        let draft = EmbedDraft {
            description: Some(&description),
            footer: Some(&footer),
            ..Default::default()
        };
        assert!(draft.build().is_err());
    }

    #[test]
    fn builds_full_embed() {
        let draft = EmbedDraft {
            title: Some("Объявление"),
            description: Some("Текст **с Markdown**"),
            image_url: Some("https://example.com/banner.png"),
            thumbnail_url: Some("https://example.com/icon.png"),
            footer: Some("Администрация"),
            color: color::DEFAULT,
            timestamp: true,
        };
        assert!(draft.build().is_ok());
    }
}
