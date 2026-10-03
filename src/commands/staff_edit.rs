//! `/staff-edit` — изменение состава администрации: выдать, снять, назначить роль.
//!
//! Правила — в [`crate::domain::hierarchy`]: изменение выполняется, только если его разрешил бы
//! Discord и вызывающий не получает через бота больше, чем может сам. План проверяется целиком
//! до первого запроса, поэтому отказ ничего не меняет. Каждое изменение пишется в системный
//! журнал и журнал аудита Discord (с указанием вызывающего).

use serenity::all::*;
use tracing::{error, info};

use crate::domain::hierarchy::{self, Hierarchy, Person, Plan, Request, mentions};
use crate::domain::text::truncate;
use crate::error::{AppError, Chain, Result};
use crate::framework::{Cx, Options, SlashCommand};
use crate::journal::{self, system};

const NAME: &str = "staff-edit";
const COLOR: Colour = Colour(0x00EB_459E);
/// Лимит Discord на `X-Audit-Log-Reason`.
const AUDIT_REASON_MAX: usize = 512;
const REASON_MAX: u16 = 400;
/// Лимит описания embed.
const DESCRIPTION_MAX: usize = 4096;
/// Код ошибки Discord «Unknown Member».
const UNKNOWN_MEMBER: isize = 10007;

pub struct StaffEdit;

#[async_trait]
impl SlashCommand for StaffEdit {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Изменить состав администрации: выдать, снять или назначить роль"
    }

    fn permission(&self) -> Permissions {
        Permissions::MANAGE_ROLES
    }

    fn options(&self) -> Vec<CreateCommandOption> {
        let member = || {
            CreateCommandOption::new(CommandOptionType::User, "member", "Участник").required(true)
        };
        let role = |description, required| {
            CreateCommandOption::new(CommandOptionType::Role, "role", description)
                .required(required)
        };
        let reason = || {
            CreateCommandOption::new(
                CommandOptionType::String,
                "reason",
                "Причина — для системного журнала и журнала аудита",
            )
            .max_length(REASON_MAX)
        };

        vec![
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "add",
                "Выдать участнику административную роль",
            )
            .add_sub_option(member())
            .add_sub_option(role("Административная роль", true))
            .add_sub_option(reason()),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "remove",
                "Снять административную роль (без роли — все)",
            )
            .add_sub_option(member())
            .add_sub_option(role(
                "Роль; без неё снимаются все административные роли",
                false,
            ))
            .add_sub_option(reason()),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "set",
                "Назначить на должность: выдать роль и снять остальные",
            )
            .add_sub_option(member())
            .add_sub_option(role("Новая административная роль", true))
            .add_sub_option(reason()),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "roles",
                "Административные роли, кто ими управляет и где роль бота",
            ),
        ]
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        let (subcommand, options) = Options::of(command)
            .subcommand()
            .ok_or_else(AppError::stale)?;
        let role = options.role("role").map(|role| role.id);
        let request = match (subcommand, role) {
            ("roles", _) => None,
            ("add", Some(role)) => Some(Request::Add(role)),
            ("remove", role) => Some(Request::Remove(role)),
            ("set", Some(role)) => Some(Request::Set(role)),
            _ => return Err(AppError::stale()),
        };
        // Участники читаются из Discord: это может не уложиться в 3 секунды на ответ.
        command.defer_ephemeral(cx).await?;

        let embed = match request {
            None => roles(cx).await?,
            Some(request) => {
                let (user, _) = options.user("member").ok_or_else(AppError::stale)?;
                edit(cx, user.id, request, options.str("reason")).await?
            }
        };
        command
            .edit_response(cx, EditInteractionResponse::new().embed(embed))
            .await?;
        Ok(())
    }
}

async fn edit(
    cx: Cx<'_>,
    target: UserId,
    request: Request,
    reason: Option<&str>,
) -> Result<CreateEmbed> {
    let guild_id = cx.caller.guild_id;
    // Роли участника и бота — из Discord: кэш без Server Members Intent может отставать.
    let member = match cx.ctx.http.get_member(guild_id, target).await {
        Ok(member) => member,
        Err(err) => {
            let err = AppError::from(err);
            return Err(if err.discord_code() == Some(UNKNOWN_MEMBER) {
                AppError::user("Пользователь не состоит на сервере.")
            } else {
                err
            });
        }
    };
    let bot = cx.bot_member().await?;

    let plan = {
        let guild = cx.guild()?;
        let caller = cx.caller.member;
        hierarchy::plan(
            &Hierarchy::of(&guild),
            Person {
                id: caller.user.id,
                roles: &caller.roles,
                bot: caller.user.bot,
            },
            Person {
                id: bot.user.id,
                roles: &bot.roles,
                bot: true,
            },
            Person {
                id: member.user.id,
                roles: &member.roles,
                bot: member.user.bot,
            },
            request,
        )
        .map_err(|refusal| AppError::user(refusal.message()))?
    };

    let author = &cx.caller.member.user;
    let audit = truncate(
        &format!(
            "{} /{NAME}: {} ({}){}",
            crate::BOT_NAME,
            author.tag(),
            author.id,
            reason.map(|r| format!(" — {r}")).unwrap_or_default()
        ),
        AUDIT_REASON_MAX,
    );
    let (done, failure) = execute(cx, target, &plan, &audit).await;

    if !done.is_empty() {
        info!(
            target: "audit",
            "/{NAME}: {} ({}) → участник {target}: выдано {:?}, снято {:?}",
            author.tag(),
            author.id,
            done.add,
            done.remove
        );
        journal::system(
            cx,
            system::staff_changed(author, target, done.add, &done.remove, reason),
        )
        .await;
    }

    let mut text = format!("Участник: {}\n", target.mention());
    if let Some(role) = done.add {
        text += &format!("➕ Выдана роль: {}\n", role.mention());
    }
    if !done.remove.is_empty() {
        text += &format!("➖ Сняты роли: {}\n", mentions(&done.remove));
    }
    let embed = CreateEmbed::new().colour(COLOR);
    match failure {
        None => Ok(embed
            .title("✅ Состав администрации изменён")
            .description(text)),
        // Ничего не сделано — обычная ошибка.
        Some(err) if done.is_empty() => Err(err),
        Some(err) => {
            error!("/{NAME}: изменение выполнено частично: {}", Chain(&err));
            text += &format!("\n⚠️ Остальное не выполнено: {}", err.user_message());
            Ok(embed
                .title("⚠️ Изменение выполнено частично")
                .description(text))
        }
    }
}

/// Выполняет план: сначала выдача, затем снятия — так участник не остаётся посередине без
/// должности. Возвращает выполненное и первую ошибку, после которой выполнение остановлено.
async fn execute(cx: Cx<'_>, target: UserId, plan: &Plan, audit: &str) -> (Plan, Option<AppError>) {
    let http = &cx.ctx.http;
    let guild = cx.caller.guild_id;
    let mut done = Plan::default();

    if let Some(role) = plan.add {
        if let Err(err) = http.add_member_role(guild, target, role, Some(audit)).await {
            return (done, Some(err.into()));
        }
        done.add = Some(role);
    }
    for &role in &plan.remove {
        if let Err(err) = http
            .remove_member_role(guild, target, role, Some(audit))
            .await
        {
            return (done, Some(err.into()));
        }
        done.remove.push(role);
    }
    (done, None)
}

/// Административные роли с отметками, кто может ими управлять, и положение роли бота.
async fn roles(cx: Cx<'_>) -> Result<CreateEmbed> {
    let standing = journal::standing(cx.ctx, cx.caller.guild_id).await?;
    let guild = cx.guild()?;
    let hierarchy = Hierarchy::of(&guild);
    let caller = cx.caller.member;
    let caller_height = hierarchy.height(Person {
        id: caller.user.id,
        roles: &caller.roles,
        bot: false,
    });

    let lines: Vec<String> = hierarchy
        .staff()
        .into_iter()
        .map(|role| {
            let mark = if !caller_height.above(role) {
                "⛔ не ниже вашей роли"
            } else if !standing.manageable.contains(&role.id) {
                "⛔ не ниже роли бота"
            } else {
                "✅"
            };
            let tier = role.tier();
            format!(
                "{mark} {} {} · {}",
                tier.emoji(),
                role.id.mention(),
                tier.label()
            )
        })
        .collect();

    let mut text = standing.describe() + "\n\n**Административные роли** (сверху вниз):\n";
    if lines.is_empty() {
        text += "не найдены.";
    }
    for (shown, line) in lines.iter().enumerate() {
        if text.chars().count() + line.chars().count() + 32 > DESCRIPTION_MAX {
            text += &format!("…и ещё {}", lines.len() - shown);
            break;
        }
        text += line;
        text.push('\n');
    }

    Ok(CreateEmbed::new()
        .colour(COLOR)
        .title("Управление администрацией")
        .description(text)
        .footer(CreateEmbedFooter::new(
            "✅ — роль можно выдавать и снимать через /staff-edit (если её права есть у вас).",
        )))
}
