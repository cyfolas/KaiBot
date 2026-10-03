//! `/perms` — политика прав сервера: роли политики, назначение каналов, аудит и синхронизация.
//!
//! Модель и планирование — в [`crate::domain::policy`]; здесь ввод, панели и исполнение плана.
//! Панели эфемерны и заменяются целиком на каждом шаге; состояние шага — в `custom_id`.
//! Применение идёт по показанному плану: кнопка несёт его отпечаток, и если сервер изменился
//! между предпросмотром и нажатием, бот покажет новый план вместо применения старого.

mod render;

use std::collections::BTreeSet;

use serenity::all::*;
use tracing::{error, info};

use self::render::{
    ACTION_APPLY, ACTION_AUDIT, ACTION_CANCEL, ACTION_CATALOG, ACTION_CHANNEL_ROLES, ACTION_PLAN,
    ACTION_ROLE, ACTION_SETUP, Nav,
};
use crate::domain::policy::{self, ChannelClass, Op, Plan, Policy, PolicyRole, Snapshot};
use crate::domain::roles::Origin;
use crate::domain::text::truncate;
use crate::error::{AppError, Result};
use crate::framework::{
    CustomId, Cx, Options, SlashCommand, ephemeral, panel, role_select, selected_roles,
};
use crate::journal::{self, system};

const NAME: &str = "perms";
/// Лимит Discord на `X-Audit-Log-Reason`.
const AUDIT_REASON_MAX: usize = 512;
/// Прогресс применения обновляется каждые столько операций.
const PROGRESS_EVERY: usize = 10;
/// Значение опции `class`, сбрасывающее явное назначение канала.
const CLASS_AUTO: &str = "auto";

pub struct Perms;

#[async_trait]
impl SlashCommand for Perms {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Политика прав: роли, назначение каналов, аудит и синхронизация"
    }

    /// Менять роли и оверрайты — и то, и другое; Discord требует все перечисленные права.
    fn permission(&self) -> Permissions {
        Permissions::MANAGE_GUILD | Permissions::MANAGE_ROLES
    }

    fn options(&self) -> Vec<CreateCommandOption> {
        let mut class =
            CreateCommandOption::new(CommandOptionType::String, "class", "Назначение канала")
                .required(true);
        for key in ChannelClass::KEYS {
            let label = ChannelClass::from_key(key, BTreeSet::new())
                .map(|class| format!("{} {}", class.emoji(), class.label()))
                .unwrap_or_default();
            class = class.add_string_choice(label, key);
        }
        class = class.add_string_choice("🧭 по оверрайтам (сбросить назначение)", CLASS_AUTO);

        vec![
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "setup",
                "Роли политики: участник, неверифицированный, чат-мут, войс-мут",
            ),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "channel",
                "Назначить класс каналу или категории",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Channel,
                    "channel",
                    "Канал или категория",
                )
                .channel_types(vec![
                    ChannelType::Text,
                    ChannelType::News,
                    ChannelType::Forum,
                    ChannelType::Voice,
                    ChannelType::Stage,
                    ChannelType::Category,
                ])
                .required(true),
            )
            .add_sub_option(class),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "audit",
                "Проверить роли и каналы на соответствие политике, ничего не меняя",
            ),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "sync",
                "План приведения сервера к политике с кнопкой применения",
            ),
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "catalog",
                "Справочник прав Discord: названия, уровни, где действуют",
            ),
        ]
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        let (subcommand, options) = Options::of(command)
            .subcommand()
            .ok_or_else(AppError::stale)?;
        match subcommand {
            "setup" => {
                let (embed, rows) = render::setup(NAME, cx.settings().policy.as_ref());
                command
                    .create_response(cx, CreateInteractionResponse::Message(panel(embed, rows)))
                    .await?;
            }
            "channel" => {
                let channel = options.channel("channel").ok_or_else(AppError::stale)?;
                let key = options.str("class").ok_or_else(AppError::stale)?;
                let response = if ChannelClass::takes_roles(key) {
                    roles_picker(cx, channel, key)?
                } else {
                    let class = (key != CLASS_AUTO)
                        .then(|| {
                            ChannelClass::from_key(key, BTreeSet::new()).ok_or_else(AppError::stale)
                        })
                        .transpose()?;
                    ephemeral(set_channel_class(cx, channel, class).await?)
                };
                command.create_response(cx, response).await?;
            }
            "audit" => {
                // Роли бота читаются из Discord: это может не уложиться в 3 секунды на ответ.
                command.defer_ephemeral(cx).await?;
                let (embed, rows) = audit_view(cx, 0).await?;
                command
                    .edit_response(
                        cx,
                        EditInteractionResponse::new().embed(embed).components(rows),
                    )
                    .await?;
            }
            "sync" => {
                command.defer_ephemeral(cx).await?;
                let (embed, rows) = plan_view(cx, 0, None).await?;
                command
                    .edit_response(
                        cx,
                        EditInteractionResponse::new().embed(embed).components(rows),
                    )
                    .await?;
            }
            "catalog" => {
                let (embed, rows) = render::catalog(NAME, &render::catalog_lines(), 0);
                command
                    .create_response(cx, CreateInteractionResponse::Message(panel(embed, rows)))
                    .await?;
            }
            _ => return Err(AppError::stale()),
        }
        Ok(())
    }

    async fn component(&self, cx: Cx<'_>, component: &ComponentInteraction) -> Result<()> {
        let mut id = CustomId::parse(&component.data.custom_id);
        match id.action {
            ACTION_ROLE => {
                let purpose =
                    PolicyRole::from_key(&id.arg::<String>()?).ok_or_else(AppError::stale)?;
                let selected = selected_roles(component).first().copied();
                set_policy_role(cx, purpose, selected).await?;
                let (embed, rows) = render::setup(NAME, cx.settings().policy.as_ref());
                component
                    .create_response(
                        cx,
                        CreateInteractionResponse::UpdateMessage(panel(embed, rows)),
                    )
                    .await?;
            }
            ACTION_CHANNEL_ROLES => {
                let channel = id.arg::<ChannelId>()?;
                let key = id.arg::<String>()?;
                let roles: BTreeSet<RoleId> = selected_roles(component).iter().copied().collect();
                let class = ChannelClass::from_key(&key, roles).ok_or_else(AppError::stale)?;
                let reply = set_channel_class(cx, channel, Some(class)).await?;
                component
                    .create_response(cx, CreateInteractionResponse::UpdateMessage(outcome(reply)))
                    .await?;
            }
            ACTION_SETUP => {
                let (embed, rows) = render::setup(NAME, cx.settings().policy.as_ref());
                component
                    .create_response(
                        cx,
                        CreateInteractionResponse::UpdateMessage(panel(embed, rows)),
                    )
                    .await?;
            }
            ACTION_AUDIT => {
                let page = id.arg::<usize>()?;
                component
                    .create_response(cx, CreateInteractionResponse::Acknowledge)
                    .await?;
                let (embed, rows) = audit_view(cx, page).await?;
                component
                    .edit_response(
                        cx,
                        EditInteractionResponse::new().embed(embed).components(rows),
                    )
                    .await?;
            }
            ACTION_PLAN => {
                let page = id.arg::<usize>()?;
                component
                    .create_response(cx, CreateInteractionResponse::Acknowledge)
                    .await?;
                let (embed, rows) = plan_view(cx, page, None).await?;
                component
                    .edit_response(
                        cx,
                        EditInteractionResponse::new().embed(embed).components(rows),
                    )
                    .await?;
            }
            ACTION_APPLY => {
                let fingerprint = id.arg::<u64>()?;
                component
                    .create_response(cx, CreateInteractionResponse::Acknowledge)
                    .await?;
                let (_, _, plan) = analyse(cx).await?;
                if plan.fingerprint() != fingerprint || plan.blocked() {
                    let notice = "⚠️ Сервер изменился после предпросмотра — проверьте новый план.";
                    let (embed, rows) = plan_view(cx, 0, Some(notice)).await?;
                    component
                        .edit_response(
                            cx,
                            EditInteractionResponse::new().embed(embed).components(rows),
                        )
                        .await?;
                    return Ok(());
                }
                apply(cx, component, &plan).await?;
            }
            ACTION_CATALOG => {
                let page = id.arg::<usize>()?;
                let (embed, rows) = render::catalog(NAME, &render::catalog_lines(), page);
                component
                    .create_response(
                        cx,
                        CreateInteractionResponse::UpdateMessage(panel(embed, rows)),
                    )
                    .await?;
            }
            ACTION_CANCEL => {
                component
                    .create_response(
                        cx,
                        CreateInteractionResponse::UpdateMessage(outcome(
                            "Применение отменено.".to_string(),
                        )),
                    )
                    .await?;
            }
            _ => return Err(AppError::stale()),
        }
        Ok(())
    }
}

/// Сообщение-итог без компонентов: заменяет панель, чтобы кнопки нельзя было нажать повторно.
fn outcome(text: String) -> CreateInteractionResponseMessage {
    CreateInteractionResponseMessage::new()
        .content(text)
        .embeds(vec![])
        .components(vec![])
        .ephemeral(true)
}

/// `/perms channel` для классов со списком ролей: панель с меню выбора; класс сохраняется
/// после выбора (см. [`ACTION_CHANNEL_ROLES`]).
fn roles_picker(cx: Cx<'_>, channel: ChannelId, key: &str) -> Result<CreateInteractionResponse> {
    let settings = cx.settings();
    let policy = settings.policy.as_ref().ok_or_else(policy_missing)?;
    let current: Vec<RoleId> = policy
        .channels
        .get(&channel)
        .and_then(ChannelClass::roles)
        .map(|roles| roles.iter().copied().collect())
        .unwrap_or_default();
    let class = ChannelClass::from_key(key, BTreeSet::new()).ok_or_else(AppError::stale)?;
    let what = match class {
        ChannelClass::Private { .. } => "роли, которым виден канал",
        _ => "роли, которые могут писать",
    };
    let embed = CreateEmbed::new()
        .colour(render::COLOR)
        .title(format!(
            "{} {} — {}",
            class.emoji(),
            channel.mention(),
            class.label()
        ))
        .description(format!(
            "Выберите {what}. Остальные роли получат доступ только на уровне сервера; \
             администраторы видят всё и так."
        ));
    let rows = vec![
        role_select(
            CustomId::encode(NAME, ACTION_CHANNEL_ROLES, &[&channel, &key]),
            "Роли…",
            &current,
            0,
            25,
        ),
        CreateActionRow::Buttons(vec![
            CreateButton::new(CustomId::encode(NAME, ACTION_CANCEL, &[]))
                .label("Отмена")
                .style(ButtonStyle::Secondary),
        ]),
    ];
    Ok(CreateInteractionResponse::Message(panel(embed, rows)))
}

fn policy_missing() -> AppError {
    AppError::user("Политика не настроена: сначала задайте роль участника в /perms setup.")
}

/// Назначает роль политики; `None` снимает назначение. Роль участника снять нельзя — без неё
/// политики нет.
async fn set_policy_role(cx: Cx<'_>, purpose: PolicyRole, role: Option<RoleId>) -> Result<()> {
    if let Some(role) = role {
        let guild = cx.guild()?;
        let info = guild
            .roles
            .get(&role)
            .ok_or_else(|| AppError::user("Роль не найдена на сервере."))?;
        let origin = Origin::of(info);
        if origin.is_managed() {
            return Err(AppError::user(format!(
                "{} — это {}: такую роль нельзя сделать ролью «{}».",
                role.mention(),
                origin.label(),
                purpose.label()
            )));
        }
    }

    let before = cx
        .state
        .settings
        .update_guild(cx.caller.guild_id, |settings| match &mut settings.policy {
            None => match (purpose, role) {
                (PolicyRole::Member, Some(role)) => {
                    settings.policy = Some(Policy::new(role));
                    Ok(None)
                }
                _ => Err(policy_missing()),
            },
            Some(policy) => {
                let before = policy.role(purpose);
                if !policy.set_role(purpose, role) {
                    return Err(AppError::user(
                        "Роль участника нельзя снять: без неё политика не имеет смысла.",
                    ));
                }
                Ok(before)
            }
        })
        .await?;
    if before == role {
        return Ok(());
    }

    let describe = |role: Option<RoleId>| {
        role.map_or_else(
            || "не задана".to_string(),
            |role| role.mention().to_string(),
        )
    };
    journal::system(
        cx,
        system::setting_changed(
            &cx.caller.member.user,
            &format!("Политика прав: роль «{}»", purpose.label()),
            &describe(before),
            &describe(role),
        ),
    )
    .await;
    Ok(())
}

/// Назначает класс каналу; `None` снимает явное назначение. Возвращает текст ответа.
async fn set_channel_class(
    cx: Cx<'_>,
    channel: ChannelId,
    class: Option<ChannelClass>,
) -> Result<String> {
    let before = cx
        .state
        .settings
        .update_guild(cx.caller.guild_id, |settings| {
            let policy = settings.policy.as_mut().ok_or_else(policy_missing)?;
            Ok(match &class {
                Some(class) => policy.channels.insert(channel, class.clone()),
                None => policy.channels.remove(&channel),
            })
        })
        .await?;

    let describe = |class: Option<&ChannelClass>| match class {
        None => "по оверрайтам".to_string(),
        Some(class) => {
            let mut text = format!("{} {}", class.emoji(), class.label());
            if let Some(roles) = class.roles()
                && !roles.is_empty()
            {
                text += &format!(
                    " ({})",
                    crate::domain::hierarchy::mentions(&roles.iter().copied().collect::<Vec<_>>())
                );
            }
            text
        }
    };
    if before != class {
        journal::system(
            cx,
            system::setting_changed(
                &cx.caller.member.user,
                &format!("Назначение канала {}", channel.mention()),
                &describe(before.as_ref()),
                &describe(class.as_ref()),
            ),
        )
        .await;
    }
    Ok(format!(
        "✅ {}: {}. Проверить результат — `/perms sync`.",
        channel.mention(),
        describe(class.as_ref())
    ))
}

/// Снимок сервера, политика и план по ней.
async fn analyse(cx: Cx<'_>) -> Result<(Snapshot, Policy, Plan)> {
    let policy = cx.settings().policy.clone().ok_or_else(policy_missing)?;
    // Роли бота — из Discord: без Server Members Intent кэш о них не узнаёт.
    let bot = cx.bot_member().await?;
    let snapshot = {
        let guild = cx.guild()?;
        Snapshot::of(&guild, &bot)
    };
    let plan = policy::plan(&snapshot, &policy);
    Ok((snapshot, policy, plan))
}

async fn audit_view(cx: Cx<'_>, page: usize) -> Result<(CreateEmbed, Vec<CreateActionRow>)> {
    let (snapshot, _, plan) = analyse(cx).await?;
    let lines = render::audit_lines(&snapshot, &plan);
    let guild_name = cx.guild()?.name.clone();
    Ok(render::audit(NAME, &guild_name, &plan, &lines, page))
}

/// План и проверка модели на состоянии, которое получится после его применения.
async fn plan_view(
    cx: Cx<'_>,
    page: usize,
    notice: Option<&str>,
) -> Result<(CreateEmbed, Vec<CreateActionRow>)> {
    let (snapshot, policy, plan) = analyse(cx).await?;
    let lines = render::plan_lines(&plan);
    let checks = if plan.blocked() {
        Vec::new()
    } else {
        let after = policy::simulate(&snapshot, &plan.ops);
        policy::verify(&after, &policy, &plan.classes)
    };
    Ok(render::plan_view(
        NAME, &plan, &lines, &checks, page, notice,
    ))
}

/// Выполняет план по порядку; останавливается на первой ошибке. Операции идемпотентны, поэтому
/// повторный `sync` продолжит с того места, где остановился.
async fn apply(cx: Cx<'_>, component: &ComponentInteraction, plan: &Plan) -> Result<()> {
    let author = &cx.caller.member.user;
    let reason = truncate(
        &format!(
            "{} /{NAME} sync: {} ({})",
            crate::BOT_NAME,
            author.tag(),
            author.id
        ),
        AUDIT_REASON_MAX,
    );
    let total = plan.ops.len();
    let mut done = 0;
    let mut failure: Option<(&Op, AppError)> = None;
    for op in &plan.ops {
        if let Err(err) = execute(cx, op, &reason).await {
            failure = Some((op, err));
            break;
        }
        done += 1;
        if done % PROGRESS_EVERY == 0 && done < total {
            let progress = EditInteractionResponse::new()
                .embed(render::progress(done, total))
                .components(vec![]);
            // Прогресс — удобство; его ошибка не должна прерывать применение.
            let _ = component.edit_response(cx, progress).await;
        }
    }

    info!(
        target: "audit",
        "/{NAME} sync: {} ({}) → сервер {}: выполнено {done} из {total}",
        author.tag(),
        author.id,
        cx.caller.guild_id
    );
    let failure_text = failure.as_ref().map(|(op, err)| {
        error!(
            "/{NAME} sync: остановлено на {op:?}: {}",
            crate::error::Chain(err)
        );
        err.user_message()
    });
    journal::system(
        cx,
        system::policy_applied(author, done, total, failure_text.as_deref()),
    )
    .await;

    let failed_op = failure.as_ref().map(|(op, _)| *op);
    let embed = render::applied(done, total, failed_op.zip(failure_text.as_deref()));
    component
        .edit_response(
            cx,
            EditInteractionResponse::new()
                .embed(embed)
                .components(vec![render::nav_row(
                    NAME,
                    &[Nav::Setup, Nav::Audit, Nav::Plan],
                )]),
        )
        .await?;
    Ok(())
}

async fn execute(cx: Cx<'_>, op: &Op, reason: &str) -> Result<()> {
    let http = &cx.ctx.http;
    match op {
        Op::EditRole { role, after, .. } => {
            cx.caller
                .guild_id
                .edit_role(
                    cx,
                    *role,
                    EditRole::new().permissions(*after).audit_log_reason(reason),
                )
                .await?;
        }
        Op::SetOverwrite {
            channel,
            role,
            allow,
            deny,
            ..
        } => {
            // Тело запроса собирается вручную: так уходит причина для журнала аудита и все биты,
            // включая неизвестные serenity.
            let body = serenity::json::json!({
                "allow": allow.bits().to_string(),
                "deny": deny.bits().to_string(),
                "type": 0,
            });
            http.create_permission(*channel, TargetId::new(role.get()), &body, Some(reason))
                .await?;
        }
        Op::DeleteOverwrite { channel, role, .. } => {
            http.delete_permission(*channel, TargetId::new(role.get()), Some(reason))
                .await?;
        }
    }
    Ok(())
}
