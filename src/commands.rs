//! Команды бота — по модулю на команду.
//!
//! Добавить команду: создать модуль с типом, реализующим [`SlashCommand`], и вписать его в [`ALL`].
//! Регистрация в Discord, маршрутизация кнопок и форм, проверка доступа и права — в `framework`.

mod access;
mod dev;
mod embed;
mod logs;
mod perms;
mod ping;
mod say;
mod staff;
mod staff_edit;

use crate::framework::{Registry, SlashCommand};

const ALL: &[&dyn SlashCommand] = &[
    &ping::Ping,
    &say::Say,
    &embed::Embed,
    &staff::Staff,
    &staff_edit::StaffEdit,
    &perms::Perms,
    &access::Access,
    &logs::Logs,
    &dev::Dev,
];

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

    /// Типы опций «подкоманда» и «группа подкоманд».
    const SUB_COMMAND: u64 = 1;
    const SUB_COMMAND_GROUP: u64 = 2;

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

    /// Опции одного уровня: команды, подкоманды или группы. Рекурсивно — для вложенных.
    fn check_options(owner: &str, options: &[Value]) {
        assert!(options.len() <= 25, "{owner}: не более 25 опций");

        let is_sub = |option: &Value| {
            matches!(
                option["type"].as_u64(),
                Some(SUB_COMMAND | SUB_COMMAND_GROUP)
            )
        };
        let subs = options.iter().filter(|option| is_sub(option)).count();
        assert!(
            subs == 0 || subs == options.len(),
            "{owner}: подкоманды нельзя смешивать с обычными опциями"
        );

        let mut names = HashSet::new();
        let mut optional_seen = false;
        for option in options {
            let name = str_field(option, "name");
            check_name(name);
            assert!(names.insert(name), "{owner}: опция «{name}» дважды");
            check_description(name, str_field(option, "description"));

            let required = option["required"].as_bool().unwrap_or(false);
            assert!(
                !(required && optional_seen),
                "{owner}: обязательная опция «{name}» после необязательной"
            );
            optional_seen |= !required;

            let choices = option["choices"].as_array().cloned().unwrap_or_default();
            assert!(choices.len() <= 25, "{owner}/{name}: не более 25 вариантов");
            for choice in &choices {
                let label = str_field(choice, "name").chars().count();
                assert!(
                    (1..=100).contains(&label),
                    "{owner}/{name}: вариант 1–100 символов"
                );
            }

            if is_sub(option) {
                let nested = option["options"].as_array().cloned().unwrap_or_default();
                check_options(&format!("{owner} {name}"), &nested);
            }
        }
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
            check_options(name, &options);
        }
    }

    #[test]
    fn developer_commands_are_hidden_from_regular_members() {
        for (command, definition) in ALL.iter().zip(registry().definitions()) {
            let json = serde_json::to_value(&definition).unwrap();
            if command.developer_only() {
                assert_eq!(
                    json["default_member_permissions"],
                    "0",
                    "{}: команды разработчиков видят только администраторы",
                    command.name()
                );
                assert!(command.permission().is_empty(), "{}", command.name());
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
