//! `/access` — доступ к боту на сервере: режим и правила для пользователей и ролей.
//!
//! Логика решения — [`crate::access`]; здесь только ввод, сохранение и системный журнал.

use serenity::all::*;

use crate::access::{Effect, GuildAccess, GuildMode, MANAGER_PERMISSIONS, RulesFull, Subject};
use crate::error::{AppError, Result};
use crate::framework::{Cx, Options, SlashCommand, ephemeral, ephemeral_embed};
use crate::journal::{self, system};
use crate::text::{describe_permissions, join_within};

const NAME: &str = "access";
const COLOR: Colour = Colour(0x0058_65F2);
/// Лимит описания embed.
const DESCRIPTION_MAX: usize = 4096;

pub struct Access;

#[async_trait]
impl SlashCommand for Access {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Доступ к боту на сервере: режим и правила"
    }

    /// Доступ к боту — настройка сервера. Тот, у кого есть это право, правилам не подчиняется.
    fn permission(&self) -> Permissions {
        Permissions::MANAGE_GUILD
    }

    fn options(&self) -> Vec<CreateCommandOption> {
        let target = |description| {
            CreateCommandOption::new(CommandOptionType::Mentionable, "target", description)
                .required(true)
        };
        let mut mode = CreateCommandOption::new(CommandOptionType::String, "mode", "Новый режим")
            .required(true);
        for value in GuildMode::ALL {
            mode = mode.add_string_choice(value.label(), value.key());
        }

        vec![
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "status",
                "Текущий режим и правила доступа",
            ),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "mode",
                "Сменить режим доступа",
            )
            .add_sub_option(mode),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "allow",
                "Выдать доступ пользователю или роли",
            )
            .add_sub_option(target("Кому выдать доступ")),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "deny",
                "Закрыть доступ пользователю или роли",
            )
            .add_sub_option(target("Кому закрыть доступ")),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "reset",
                "Снять правило: доступ снова определяет режим",
            )
            .add_sub_option(target("Чьё правило снять")),
        ]
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        let (subcommand, options) = Options::of(command)
            .subcommand()
            .ok_or_else(AppError::stale)?;

        let response = match subcommand {
            "status" => ephemeral_embed(
                CreateEmbed::new()
                    .colour(COLOR)
                    .title("Доступ к боту")
                    .description(status(&cx.settings().access)),
            ),
            "mode" => ephemeral(set_mode(cx, &options).await?),
            "allow" => ephemeral(set_rule(cx, &options, Some(Effect::Allow)).await?),
            "deny" => ephemeral(set_rule(cx, &options, Some(Effect::Deny)).await?),
            "reset" => ephemeral(set_rule(cx, &options, None).await?),
            _ => return Err(AppError::stale()),
        };
        command.create_response(cx, response).await?;
        Ok(())
    }
}

async fn set_mode(cx: Cx<'_>, options: &Options<'_>) -> Result<String> {
    let mode = options
        .str("mode")
        .and_then(GuildMode::from_key)
        .ok_or_else(AppError::stale)?;
    let before = cx
        .state
        .settings
        .update_guild(cx.caller.guild_id, |settings| {
            Ok(std::mem::replace(&mut settings.access.mode, mode))
        })
        .await?;
    if before == mode {
        return Ok(format!("Режим уже «{}».", mode.label()));
    }

    journal::system(
        cx,
        system::setting_changed(
            &cx.caller.member.user,
            "Режим доступа к боту",
            before.label(),
            mode.label(),
        ),
    )
    .await;

    let mut reply = format!("✅ Режим доступа: «{}» — {}.", mode.label(), mode.explain());
    let has_grants = cx
        .settings()
        .access
        .rules()
        .values()
        .any(|&effect| effect == Effect::Allow);
    if mode == GuildMode::Restricted && !has_grants {
        reply += "\n⚠️ Доступ пока никому не выдан: ботом может пользоваться только администрация. \
                  Выдайте доступ: `/access allow`.";
    }
    Ok(reply)
}

/// Устанавливает (`Some`) или снимает (`None`) правило.
async fn set_rule(cx: Cx<'_>, options: &Options<'_>, effect: Option<Effect>) -> Result<String> {
    let (subject, exempt) = target(cx, options)?;
    let before = cx
        .state
        .settings
        .update_guild(cx.caller.guild_id, |settings| match effect {
            Some(effect) => settings.access.set_rule(subject, effect).map_err(|RulesFull| {
                AppError::user(format!(
                    "Правил уже {}: снимите ненужные (`/access reset`), прежде чем добавлять новые.",
                    crate::access::MAX_RULES
                ))
            }),
            None => Ok(settings.access.remove_rule(subject)),
        })
        .await?;

    if before == effect {
        return Ok(format!(
            "Для {} уже {}.",
            subject.mention(),
            describe_rule(effect)
        ));
    }
    journal::system(
        cx,
        system::setting_changed(
            &cx.caller.member.user,
            "Правило доступа к боту",
            &format!("{} — {}", subject.mention(), describe_rule(before)),
            &format!("{} — {}", subject.mention(), describe_rule(effect)),
        ),
    )
    .await;

    let mut reply = match effect {
        Some(Effect::Allow) => format!("✅ Доступ выдан: {}.", subject.mention()),
        Some(Effect::Deny) => format!("✅ Доступ закрыт: {}.", subject.mention()),
        None => format!(
            "✅ Правило для {} снято: доступ определяет режим.",
            subject.mention()
        ),
    };
    if let Some(note) = exempt {
        reply += &format!("\n⚠️ {note}");
    }
    let mode = cx.settings().access.mode;
    if mode == GuildMode::Locked && effect.is_some() {
        reply += "\nℹ️ Сейчас режим «Только администрация»: правила начнут действовать после смены \
                  режима.";
    }
    Ok(reply)
}

/// Пользователь или роль из опции `target` и предупреждение, если правило на них не подействует.
fn target(cx: Cx<'_>, options: &Options<'_>) -> Result<(Subject, Option<String>)> {
    if let Some(role) = options.role("target") {
        if role.id.get() == cx.caller.guild_id.get() {
            return Err(AppError::user(
                "@everyone — это все участники: для них используйте режим (`/access mode`).",
            ));
        }
        let exempt = role.permissions.intersects(MANAGER_PERMISSIONS).then(|| {
            format!(
                "У роли есть {}: её участники управляют ботом, и правила на них не действуют.",
                describe_permissions(role.permissions & MANAGER_PERMISSIONS)
            )
        });
        return Ok((Subject::Role(role.id), exempt));
    }

    if let Some((user, member)) = options.user("target") {
        if user.bot {
            return Err(AppError::user(
                "Боты не вызывают команды — правило для бота ничего не изменит.",
            ));
        }
        let permissions = member.and_then(|member| member.permissions);
        let exempt = permissions
            .filter(|granted| granted.intersects(MANAGER_PERMISSIONS))
            .map(|granted| {
                format!(
                    "У участника есть {}: он управляет ботом, и правила на него не действуют.",
                    describe_permissions(granted & MANAGER_PERMISSIONS)
                )
            });
        return Ok((Subject::User(user.id), exempt));
    }

    Err(AppError::stale())
}

fn describe_rule(effect: Option<Effect>) -> &'static str {
    match effect {
        Some(Effect::Allow) => "доступ выдан",
        Some(Effect::Deny) => "доступ закрыт",
        None => "нет правила",
    }
}

/// Режим и правила сервера. Не длиннее лимита описания embed.
fn status(access: &GuildAccess) -> String {
    let list = |wanted: Effect| -> Vec<String> {
        access
            .rules()
            .iter()
            .filter(|&(_, &effect)| effect == wanted)
            .map(|(subject, _)| subject.mention())
            .collect()
    };
    let (allowed, denied) = (list(Effect::Allow), list(Effect::Deny));

    let mut text = format!(
        "**Режим:** {} — {}.\n",
        access.mode.label(),
        access.mode.explain()
    );
    let footer = "\n\nАдминистрация (право «Управлять сервером») правилам не подчиняется. \
                  Правило пользователя важнее правил его ролей; среди ролей запрет важнее \
                  разрешения.";
    // Списки делят оставшееся место поровну; не поместившиеся заменяются счётчиком.
    let budget = (DESCRIPTION_MAX - text.chars().count() - footer.chars().count() - 64) / 2;
    for (title, items) in [("Доступ выдан", allowed), ("Доступ закрыт", denied)]
    {
        let joined = if items.is_empty() {
            "—".to_string()
        } else {
            join_within(&items, budget)
        };
        text += &format!("\n**{title}:** {joined}");
    }
    text + footer
}

#[cfg(test)]
mod tests {
    use serenity::all::{RoleId, UserId};

    use super::*;
    use crate::access::MAX_RULES;

    #[test]
    fn status_fits_embed_with_maximum_rules() {
        let mut access = GuildAccess::default();
        for id in 0..MAX_RULES as u64 {
            let subject = if id % 2 == 0 {
                Subject::User(UserId::new(u64::MAX - id))
            } else {
                Subject::Role(RoleId::new(u64::MAX - id))
            };
            let effect = if id % 3 == 0 {
                Effect::Deny
            } else {
                Effect::Allow
            };
            access.set_rule(subject, effect).unwrap();
        }
        let text = status(&access);
        assert!(text.chars().count() <= DESCRIPTION_MAX, "{}", text.len());
    }

    #[test]
    fn status_lists_rules_by_effect() {
        let mut access = GuildAccess::default();
        access.mode = GuildMode::Restricted;
        access
            .set_rule(Subject::Role(RoleId::new(5)), Effect::Allow)
            .unwrap();
        let text = status(&access);
        assert!(text.contains("По списку"));
        assert!(text.contains("**Доступ выдан:** <@&5>"));
        assert!(text.contains("**Доступ закрыт:** —"));
    }
}
