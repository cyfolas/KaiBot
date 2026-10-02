//! Текст для Discord: названия прав по-русски и укладывание в лимиты сообщений и embed.
//! Чистая логика — покрыта тестами.

use serenity::all::Permissions;

/// Запас под « и ещё N» в конце списка при любом разумном N.
const SUFFIX_RESERVE: usize = 16;

/// Названия прав, как в русском клиенте Discord. Для прочих — английские названия serenity.
const PERMISSION_NAMES: &[(Permissions, &str)] = &[
    (Permissions::ADMINISTRATOR, "Администратор"),
    (Permissions::MANAGE_GUILD, "Управлять сервером"),
    (Permissions::MANAGE_ROLES, "Управлять ролями"),
    (Permissions::MANAGE_CHANNELS, "Управлять каналами"),
    (Permissions::MANAGE_MESSAGES, "Управлять сообщениями"),
    (Permissions::KICK_MEMBERS, "Выгонять участников"),
    (Permissions::BAN_MEMBERS, "Банить участников"),
    (Permissions::MODERATE_MEMBERS, "Тайм-аут участников"),
    (Permissions::VIEW_CHANNEL, "Просматривать каналы"),
    (Permissions::SEND_MESSAGES, "Отправлять сообщения"),
    (
        Permissions::SEND_MESSAGES_IN_THREADS,
        "Отправлять сообщения в ветках",
    ),
    (Permissions::EMBED_LINKS, "Встраивать ссылки"),
    (Permissions::ATTACH_FILES, "Прикреплять файлы"),
    (
        Permissions::MENTION_EVERYONE,
        "Упоминание @everyone, @here и всех ролей",
    ),
];

/// «Управлять сервером», «Встраивать ссылки» — в порядке битов.
pub fn describe_permissions(permissions: Permissions) -> String {
    permissions
        .iter()
        .map(
            |flag| match PERMISSION_NAMES.iter().find(|(known, _)| *known == flag) {
                Some((_, name)) => format!("«{name}»"),
                None => format!("«{}»", flag.get_permission_names().join(", ")),
            },
        )
        .collect::<Vec<_>>()
        .join(", ")
}

/// Склеивает элементы через «, », укладываясь в `max` символов; не поместившиеся заменяются
/// на «и ещё N».
pub fn join_within(items: &[String], max: usize) -> String {
    let mut out = String::new();
    let mut len = 0;
    for (i, item) in items.iter().enumerate() {
        let added = item.chars().count() + if i == 0 { 0 } else { 2 };
        let is_last = i + 1 == items.len();
        let limit = if is_last {
            max
        } else {
            max.saturating_sub(SUFFIX_RESERVE)
        };

        if len + added > limit {
            let separator = if i == 0 { "" } else { " " };
            out += &format!("{separator}и ещё {}", items.len() - i);
            break;
        }
        if i > 0 {
            out += ", ";
        }
        out += item;
        len += added;
    }
    out
}

/// Не длиннее `max` символов: лишнее заменяется многоточием.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("<@{:018}>", i + 1)).collect()
    }

    #[test]
    fn describes_permissions_in_russian() {
        assert_eq!(
            describe_permissions(Permissions::EMBED_LINKS | Permissions::MANAGE_GUILD),
            "«Управлять сервером», «Встраивать ссылки»"
        );
    }

    #[test]
    fn falls_back_to_serenity_names() {
        assert_eq!(
            describe_permissions(Permissions::MANAGE_WEBHOOKS),
            "«Manage Webhooks»"
        );
    }

    #[test]
    fn joins_everything_that_fits() {
        assert_eq!(
            join_within(&items(2), 100),
            "<@000000000000000001>, <@000000000000000002>"
        );
    }

    #[test]
    fn truncates_with_counter_and_respects_limit() {
        let all = items(100);
        for max in [16, 30, 100, 1024] {
            let joined = join_within(&all, max);
            assert!(joined.chars().count() <= max, "max={max}: {joined}");
            let shown = joined.matches("<@").count();
            assert!(
                joined.ends_with(&format!("и ещё {}", 100 - shown)),
                "{joined}"
            );
        }
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("привет", 10), "привет");
        assert_eq!(truncate("привет", 4), "при…");
        assert_eq!(truncate("привет", 4).chars().count(), 4);
    }
}
