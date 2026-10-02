//! Какие упоминания в сообщении от имени бота присылают уведомления.
//!
//! У бота права администратора, и без ограничений он уведомил бы кого угодно. Поэтому
//! уведомления ограничены правами автора — так же, как если бы он писал сам:
//! * пользователей может уведомить любой;
//! * роль — только если она упоминаемая (`mentionable`);
//! * @everyone, @here и неупоминаемые роли — только с правом «Упоминание @everyone, @here и
//!   всех ролей» в целевом канале.

use std::collections::BTreeSet;

use serenity::all::{CreateAllowedMentions, Permissions, RoleId};

/// Лимит Discord на число ролей в `allowed_mentions.roles`.
const MAX_ROLES: usize = 100;

/// Упоминания отображаются, но никого не уведомляют.
pub fn silent() -> CreateAllowedMentions {
    CreateAllowedMentions::new()
}

/// Уведомления, которые автор с правами `granted` мог бы вызвать сам.
pub fn allowed(
    text: &str,
    granted: Permissions,
    mentionable: impl Fn(RoleId) -> bool,
) -> CreateAllowedMentions {
    let mentions = silent().all_users(true);
    if granted.mention_everyone() {
        return mentions.everyone(true).all_roles(true);
    }

    let roles: Vec<RoleId> = mentioned_roles(text)
        .into_iter()
        .filter(|&role| mentionable(role))
        .take(MAX_ROLES)
        .collect();
    mentions.roles(roles)
}

/// Роли, упомянутые в тексте как `<@&ID>`, без повторов.
fn mentioned_roles(text: &str) -> BTreeSet<RoleId> {
    text.split("<@&")
        .skip(1)
        .filter_map(|rest| {
            let (id, _) = rest.split_once('>')?;
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            id.parse().ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn to_json(mentions: CreateAllowedMentions) -> Value {
        serde_json::to_value(mentions).unwrap()
    }

    #[test]
    fn finds_role_mentions() {
        let roles = mentioned_roles("<@&1> <@1> <@&2><@&1> <@&> <@&x> <@&+3> <@&4");
        assert_eq!(roles, BTreeSet::from([RoleId::new(1), RoleId::new(2)]));
    }

    #[test]
    fn silent_notifies_nobody() {
        let json = to_json(silent());
        assert_eq!(json["parse"], json!([]));
        assert_eq!(json["roles"], json!([]));
        assert_eq!(json["users"], json!([]));
    }

    #[test]
    fn without_mention_everyone_only_mentionable_roles_ping() {
        let json = to_json(allowed(
            "<@&1> <@&2> @everyone",
            Permissions::SEND_MESSAGES,
            |role| role == RoleId::new(2),
        ));
        assert_eq!(json["parse"], json!(["users"]));
        assert_eq!(json["roles"], json!(["2"]));
    }

    #[test]
    fn mention_everyone_allows_everything() {
        let json = to_json(allowed("<@&1>", Permissions::MENTION_EVERYONE, |_| false));
        let parse = json["parse"].as_array().unwrap();
        for kind in ["users", "roles", "everyone"] {
            assert!(parse.contains(&json!(kind)), "{kind}");
        }
    }
}
