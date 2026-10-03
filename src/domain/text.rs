//! Текст для Discord: укладывание списков и строк в лимиты сообщений и embed.
//! Чистая логика — покрыта тестами. Названия прав — в [`super::permissions`].

use serenity::all::Permissions;

/// Запас под « и ещё N» в конце списка при любом разумном N.
const SUFFIX_RESERVE: usize = 16;

/// «Управлять сервером», «Встраивать ссылки» — в порядке битов (см. [`super::permissions::describe`]).
pub fn describe_permissions(permissions: Permissions) -> String {
    super::permissions::describe(permissions)
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

/// Строки, уложенные в `max` символов с переводами строк; не поместившиеся заменяются
/// строкой «…и ещё N».
pub fn lines_within(lines: &[String], max: usize) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let is_last = i + 1 == lines.len();
        let limit = if is_last {
            max
        } else {
            max.saturating_sub(SUFFIX_RESERVE)
        };
        let added = line.chars().count() + usize::from(i > 0);
        if out.chars().count() + added > limit {
            if i > 0 {
                out.push('\n');
            }
            out += &format!("…и ещё {}", lines.len() - i);
            break;
        }
        if i > 0 {
            out.push('\n');
        }
        out += line;
    }
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

    #[test]
    fn lines_within_respects_limit() {
        let lines: Vec<String> = (0..50).map(|i| format!("строка {i}")).collect();
        for max in [20, 60, 200, 4096] {
            let text = lines_within(&lines, max);
            assert!(text.chars().count() <= max, "max={max}: {text}");
        }
        assert_eq!(lines_within(&lines[..2], 100), "строка 0\nстрока 1");
        assert!(lines_within(&lines, 60).contains("…и ещё "));
    }
}
