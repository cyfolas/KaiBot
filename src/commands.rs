//! Команды бота — по модулю на команду.
//!
//! Добавить команду: создать модуль с типом, реализующим [`SlashCommand`], и вписать его в [`ALL`].
//! Регистрация в Discord, маршрутизация кнопок и форм и проверка права — в `framework`.

mod embed;
mod ping;
mod say;
mod staff;

use crate::framework::{Registry, SlashCommand};

const ALL: &[&dyn SlashCommand] = &[&ping::Ping, &say::Say, &embed::Embed, &staff::Staff];

pub fn registry() -> Registry {
    Registry::new(ALL)
}

/// Ограничения Discord на определения команд проверяются тестом, а не при регистрации:
/// иначе ошибка проявилась бы только при запуске бота (код 50035).
/// <https://discord.com/developers/docs/interactions/application-commands#application-command-object>
#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::Value;

    use super::*;

    fn check_name(name: &str) {
        let len = name.chars().count();
        assert!((1..=32).contains(&len), "имя «{name}»: 1–32 символа");
        assert!(
            name.chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_'),
            "имя «{name}»: только буквы, цифры, - и _"
        );
        assert_eq!(name, name.to_lowercase(), "имя «{name}»: только строчные");
    }

    fn check_description(owner: &str, description: &str) {
        let len = description.chars().count();
        assert!((1..=100).contains(&len), "{owner}: описание 1–100 символов");
    }

    fn str_field<'a>(value: &'a Value, field: &str) -> &'a str {
        value[field].as_str().unwrap_or_default()
    }

    #[test]
    fn definitions_satisfy_discord_limits() {
        let mut names = HashSet::new();
        for definition in registry().definitions() {
            let json = serde_json::to_value(&definition).unwrap();
            let name = str_field(&json, "name");
            check_name(name);
            assert!(
                names.insert(name.to_owned()),
                "команда «{name}» объявлена дважды"
            );
            check_description(name, str_field(&json, "description"));

            let options = json["options"].as_array().cloned().unwrap_or_default();
            assert!(options.len() <= 25, "{name}: не более 25 опций");

            let mut option_names = HashSet::new();
            let mut optional_seen = false;
            for option in &options {
                let option_name = str_field(option, "name");
                check_name(option_name);
                assert!(
                    option_names.insert(option_name),
                    "{name}: опция «{option_name}» дважды"
                );
                check_description(option_name, str_field(option, "description"));

                let required = option["required"].as_bool().unwrap_or(false);
                assert!(
                    !(required && optional_seen),
                    "{name}: обязательная опция «{option_name}» после необязательной"
                );
                optional_seen |= !required;
            }
        }
    }

    #[test]
    fn registry_finds_every_command_by_name() {
        let registry = registry();
        for command in ALL {
            assert_eq!(registry.get(command.name()).unwrap().name(), command.name());
        }
        assert!(registry.get("missing").is_none());
    }
}
