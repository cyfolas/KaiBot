//! Текст `/staff`. Чистая логика без обращений к Discord — покрыта тестами.

use serenity::all::{Mentionable, RoleId, UserId};

use crate::domain::roles::Tier;
use crate::domain::text::join_within;

/// Лимит описания embed.
const DESCRIPTION_MAX: usize = 4096;
/// Запас под итоговую строку «…и ещё ролей: N», если не поместились все группы.
const TAIL_RESERVE: usize = 32;
/// Минимум места под заголовком группы: «и ещё N» и перевод строки.
const GROUP_MIN: usize = 24;

/// Административная роль и участники, для которых она высшая.
pub struct StaffRole {
    pub id: RoleId,
    pub tier: Tier,
    pub members: Vec<UserId>,
}

/// Описание embed: владелец, разработчики, затем роли по уровням (администраторы, управление,
/// модерация), внутри уровня — по старшинству. Не длиннее лимита Discord: не поместившиеся
/// участники и роли заменяются счётчиками.
pub fn render(owner_id: UserId, developers: &[UserId], roles: &[StaffRole]) -> String {
    let mut text = format!("👑 **Владелец сервера:** {}\n", owner_id.mention());
    if !developers.is_empty() {
        let mentions: Vec<String> = developers
            .iter()
            .map(|id| id.mention().to_string())
            .collect();
        text += &format!("🛠 **Разработчики бота:** {}\n", mentions.join(", "));
    }

    let mut groups: Vec<&StaffRole> = roles
        .iter()
        .filter(|role| !role.members.is_empty())
        .collect();
    // Устойчивая сортировка: порядок по старшинству внутри уровня сохраняется.
    groups.sort_by_key(|role| role.tier);
    if groups.is_empty() {
        text += "\nАдминистративные роли не найдены.";
        return text;
    }

    let budget = DESCRIPTION_MAX - TAIL_RESERVE;
    let mut current_tier = None;
    for (shown, role) in groups.iter().enumerate() {
        let mut header = String::new();
        if current_tier != Some(role.tier) {
            current_tier = Some(role.tier);
            header += &format!("\n{} **{}**", role.tier.emoji(), role.tier.label());
        }
        header += &format!("\n{}\n", role.id.mention());

        let used = text.chars().count() + header.chars().count();
        if used + GROUP_MIN > budget {
            text += &format!("\n…и ещё ролей: {}", groups.len() - shown);
            break;
        }

        let mentions: Vec<String> = role
            .members
            .iter()
            .map(|id| id.mention().to_string())
            .collect();
        text += &header;
        text += &join_within(&mentions, budget - used - 1);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_exceeds_description_limit() {
        let roles: Vec<StaffRole> = (1..=40)
            .map(|r| StaffRole {
                id: RoleId::new(r),
                tier: if r == 1 {
                    Tier::Administrator
                } else {
                    Tier::Moderation
                },
                members: (1..=200).map(|m| UserId::new(r * 1_000 + m)).collect(),
            })
            .collect();
        let text = render(UserId::new(7), &[UserId::new(8)], &roles);
        assert!(text.chars().count() <= DESCRIPTION_MAX);
        assert!(text.contains("…и ещё ролей"));
    }

    #[test]
    fn groups_by_tier_then_seniority_and_skips_empty() {
        let role = |id, tier, members: Vec<u64>| StaffRole {
            id: RoleId::new(id),
            tier,
            members: members.into_iter().map(UserId::new).collect(),
        };
        // Позиции сверху вниз: 1 (модерация), 2 (пусто), 3 (администратор), 4 (модерация).
        let roles = vec![
            role(1, Tier::Moderation, vec![10]),
            role(2, Tier::Management, vec![]),
            role(3, Tier::Administrator, vec![30]),
            role(4, Tier::Moderation, vec![40]),
        ];
        let text = render(UserId::new(99), &[UserId::new(10)], &roles);

        assert!(text.contains("<@99>"));
        assert!(!text.contains("<@&2>"));
        assert!(!text.contains("Управление"));
        let admin = text.find("<@&3>").unwrap();
        let first_mod = text.find("<@&1>").unwrap();
        let second_mod = text.find("<@&4>").unwrap();
        assert!(admin < first_mod && first_mod < second_mod);
        assert!(text.find("Администраторы").unwrap() < text.find("Модерация").unwrap());
    }

    #[test]
    fn reports_missing_staff() {
        let text = render(UserId::new(1), &[], &[]);
        assert!(text.contains("не найдены"));
        assert!(!text.contains("Разработчики"));
    }
}
