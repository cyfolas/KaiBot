//! `/staff` — администрация сервера по данным Discord в реальном времени.
//!
//! Никаких зашитых ID: административной считается любая роль с правами управления (см.
//! [`crate::hierarchy`]), порядок групп — позиции ролей на сервере, владелец — владелец сервера
//! по данным Discord. Роли интеграций (в том числе роль самого бота) и роли-«разделители» без
//! прав не учитываются. Каждый участник показывается один раз — под своей самой высокой такой
//! ролью. Изменить состав — `/staff-edit`.

mod render;

use std::collections::HashMap;

use serenity::all::*;

use self::render::StaffRole;
use crate::error::{AppError, Result};
use crate::framework::{Cx, Options, SlashCommand};
use crate::hierarchy::Hierarchy;

/// Максимальный размер страницы `GET /guilds/{id}/members`.
const MEMBERS_PAGE: u64 = 1000;

pub struct Staff;

#[async_trait]
impl SlashCommand for Staff {
    fn name(&self) -> &'static str {
        "staff"
    }

    fn description(&self) -> &'static str {
        "Администрация сервера и разработчики бота"
    }

    fn options(&self) -> Vec<CreateCommandOption> {
        vec![CreateCommandOption::new(
            CommandOptionType::Boolean,
            "public",
            "Показать ответ всем в канале (по умолчанию — только вам)",
        )]
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        // Загрузка участников может не уложиться в 3 секунды, отведённые на ответ.
        if Options::of(command).bool("public").unwrap_or(false) {
            command.defer(cx).await?;
        } else {
            command.defer_ephemeral(cx).await?;
        }

        let (guild_name, owner_id, mut roles) = staff_roles(&cx)?;
        assign_members(cx, &mut roles).await?;

        let embed = CreateEmbed::new()
            .title(format!("Администрация · {guild_name}"))
            .colour(Colour::new(0x0058_65F2))
            .description(render::render(owner_id, cx.state.developers(), &roles))
            .footer(CreateEmbedFooter::new(
                "Роли с правами управления, по старшинству. Каждый участник — под своей высшей ролью.",
            ));

        command
            .edit_response(cx, EditInteractionResponse::new().embed(embed))
            .await?;
        Ok(())
    }
}

/// Название сервера, владелец и административные роли от старшей к младшей — из кэша.
fn staff_roles(cx: &Cx<'_>) -> Result<(String, UserId, Vec<StaffRole>)> {
    let guild = cx.guild()?;
    let roles = Hierarchy::of(&guild)
        .staff()
        .into_iter()
        .map(|role| StaffRole {
            id: role.id,
            administrator: role.permissions.administrator(),
            members: Vec::new(),
        })
        .collect();

    Ok((guild.name.clone(), guild.owner_id, roles))
}

/// Распределяет участников (кроме ботов) по их высшей административной роли.
///
/// Участники читаются постранично и целиком не хранятся. Сложность — O(Σ ролей участников),
/// плюс ⌈N / 1000⌉ запросов к API.
async fn assign_members(cx: Cx<'_>, roles: &mut [StaffRole]) -> Result<()> {
    // Индекс в `roles` — ранг роли: меньше значит старше.
    let rank: HashMap<RoleId, usize> = roles
        .iter()
        .enumerate()
        .map(|(i, role)| (role.id, i))
        .collect();

    let mut after = None;
    loop {
        let page = cx
            .caller
            .guild_id
            .members(cx, Some(MEMBERS_PAGE), after)
            .await
            .map_err(members_error)?;

        for member in page.iter().filter(|member| !member.user.bot) {
            if let Some(&top) = member.roles.iter().filter_map(|id| rank.get(id)).min() {
                roles[top].members.push(member.user.id);
            }
        }

        // Неполная страница — последняя; участники идут по возрастанию ID.
        match page.last() {
            Some(last) if page.len() as u64 == MEMBERS_PAGE => after = Some(last.user.id),
            _ => return Ok(()),
        }
    }
}

fn members_error(err: serenity::Error) -> AppError {
    let err = AppError::from(err);
    if err.discord_code() == Some(50001) {
        AppError::user(
            "Боту недоступен список участников: включите Server Members Intent \
             в Discord Developer Portal → Bot → Privileged Gateway Intents.",
        )
    } else {
        err
    }
}
