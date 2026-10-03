//! Текст и компоненты панелей `/perms`. Чистая логика — покрыта тестами.

use std::fmt::Display;

use serenity::all::*;

use crate::domain::permissions::describe;
use crate::domain::policy::{
    ChannelClass, Check, Op, Plan, Policy, PolicyRole, Severity, Snapshot,
};
use crate::domain::text::{lines_within, truncate};
use crate::framework::{CustomId, Page, confirm_row, emoji};

pub const COLOR: Colour = Colour(0x001A_BC9C);
const COLOR_WARNING: Colour = Colour(0x00FE_E75C);
const COLOR_BLOCKED: Colour = Colour(0x00ED_4245);

/// Лимит описания embed и строк на страницу.
const DESCRIPTION_MAX: usize = 4000;
const PER_PAGE: usize = 14;
/// Одна строка списка не длиннее этого: названия прав бывают длинными.
const LINE_MAX: usize = 300;

pub const ACTION_ROLE: &str = "role";
pub const ACTION_CHANNEL_ROLES: &str = "chroles";
pub const ACTION_AUDIT: &str = "audit";
pub const ACTION_PLAN: &str = "plan";
pub const ACTION_APPLY: &str = "apply";
pub const ACTION_CANCEL: &str = "cancel";
pub const ACTION_SETUP: &str = "setup";
pub const ACTION_CATALOG: &str = "catalog";

/// Панель настройки: роли политики и переходы к аудиту и плану.
pub fn setup(command: &str, policy: Option<&Policy>) -> (CreateEmbed, Vec<CreateActionRow>) {
    let mut description = String::from(
        "Модель: @everyone не даёт ничего, роль участника несёт базовые права на уровне роли, \
         каналы нейтральны, роли мутов запрещают в каждом канале. Выберите роли ниже — изменения \
         сохраняются сразу.\n",
    );
    for purpose in PolicyRole::ALL {
        let value = match policy.and_then(|policy| policy.role(purpose)) {
            Some(role) => role.mention().to_string(),
            None => "*не задана*".to_string(),
        };
        description += &format!(
            "\n**{}** — {}\n-# {}",
            purpose.label(),
            value,
            purpose.explain()
        );
    }
    if policy.is_none() {
        description += "\n\n⚠️ Политика не настроена: начните с роли участника.";
    }

    let mut rows: Vec<CreateActionRow> = PolicyRole::ALL
        .into_iter()
        .map(|purpose| {
            let current: Vec<RoleId> = policy
                .and_then(|policy| policy.role(purpose))
                .into_iter()
                .collect();
            let min = u8::from(purpose == PolicyRole::Member);
            crate::framework::role_select(
                CustomId::encode(command, ACTION_ROLE, &[&purpose.key()]),
                &format!("{} — выбрать роль", purpose.label()),
                &current,
                min,
                1,
            )
        })
        .collect();
    rows.push(nav_row(command, &[Nav::Audit, Nav::Plan]));

    let embed = CreateEmbed::new()
        .colour(COLOR)
        .title("⚙️ Политика прав")
        .description(description)
        .footer(CreateEmbedFooter::new(
            "Назначение каналов: /perms channel. Проверка: Аудит. Применение: План → Применить.",
        ));
    (embed, rows)
}

/// Кнопки перехода между панелями.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Setup,
    Audit,
    Plan,
}

pub fn nav_row(command: &str, items: &[Nav]) -> CreateActionRow {
    CreateActionRow::Buttons(
        items
            .iter()
            .map(|item| match item {
                Nav::Setup => CreateButton::new(CustomId::encode(command, ACTION_SETUP, &[]))
                    .label("Настройка")
                    .emoji(emoji("⚙️"))
                    .style(ButtonStyle::Secondary),
                Nav::Audit => CreateButton::new(CustomId::encode(command, ACTION_AUDIT, &[&0]))
                    .label("Аудит")
                    .emoji(emoji("🔍"))
                    .style(ButtonStyle::Primary),
                Nav::Plan => CreateButton::new(CustomId::encode(command, ACTION_PLAN, &[&0]))
                    .label("План изменений")
                    .emoji(emoji("🔄"))
                    .style(ButtonStyle::Primary),
            })
            .collect(),
    )
}

/// Строки аудита: замечания по убыванию важности, затем каналы по категориям с их классами.
pub fn audit_lines(snapshot: &Snapshot, plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();
    let mut findings = plan.findings.clone();
    findings.sort_by_key(|finding| std::cmp::Reverse(finding.severity));
    if findings.is_empty() {
        lines.push("✅ Замечаний нет.".to_string());
    } else {
        lines.push(format!("**Замечания ({}):**", findings.len()));
        lines.extend(
            findings
                .iter()
                .map(|finding| format!("{} {}", finding.severity.emoji(), finding.text)),
        );
    }

    lines.push(String::new());
    lines.push(format!("**Каналы ({}):**", plan.classes.len()));
    let describe_class = |class: &ChannelClass| -> String {
        let mut text = format!("{} {}", class.emoji(), class.label());
        if let Some(roles) = class.roles()
            && !roles.is_empty()
        {
            let mentions: Vec<String> = roles
                .iter()
                .map(|role| role.mention().to_string())
                .collect();
            text += &format!(" · {}", mentions.join(", "));
        }
        text
    };
    // Снимок уже упорядочен: категории, затем каналы по категории и позиции.
    let mut without_parent = Vec::new();
    for channel in &snapshot.channels {
        let Some(class) = plan.classes.get(&channel.id) else {
            continue;
        };
        if channel.kind == ChannelType::Category {
            lines.push(format!(
                "📁 {} — {}",
                channel.id.mention(),
                describe_class(class)
            ));
        } else if channel.parent.is_some() {
            lines.push(format!(
                "　└ {} — {}",
                channel.id.mention(),
                describe_class(class)
            ));
        } else {
            without_parent.push(format!(
                "{} — {}",
                channel.id.mention(),
                describe_class(class)
            ));
        }
    }
    lines.extend(without_parent);
    lines
}

/// Страница аудита.
pub fn audit(
    command: &str,
    guild_name: &str,
    plan: &Plan,
    lines: &[String],
    requested: usize,
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let page = Page::of(lines.len(), PER_PAGE, requested);
    let shown: Vec<String> = page
        .slice(lines, PER_PAGE)
        .iter()
        .map(|line| truncate(line, LINE_MAX))
        .collect();
    let embed = CreateEmbed::new()
        .colour(if plan.blocked() { COLOR_BLOCKED } else { COLOR })
        .title(format!("🔍 Аудит прав · {guild_name}"))
        .description(lines_within(&shown, DESCRIPTION_MAX))
        .footer(CreateEmbedFooter::new(format!(
            "Операций в плане: {} · {}",
            plan.ops.len(),
            page.label()
        )));
    let mut rows = Vec::new();
    rows.extend(page.buttons(command, ACTION_AUDIT, &[]));
    rows.push(nav_row(command, &[Nav::Setup, Nav::Plan]));
    (embed, rows)
}

/// Строка операции плана.
pub fn op_line(op: &Op) -> String {
    match op {
        Op::EditRole {
            role,
            before,
            after,
        } => {
            let removed = *before - *after;
            let added = *after - *before;
            let mut text = format!("🛡️ {}:", role.mention());
            if !removed.is_empty() {
                text += &format!(" снять {}", describe(removed));
            }
            if !added.is_empty() {
                let separator = if removed.is_empty() { "" } else { ";" };
                text += &format!("{separator} выдать {}", describe(added));
            }
            text
        }
        Op::SetOverwrite {
            role, allow, deny, ..
        } => {
            let mut parts = Vec::new();
            if !allow.is_empty() {
                parts.push(format!("✅ {}", describe(*allow)));
            }
            if !deny.is_empty() {
                parts.push(format!("⛔ {}", describe(*deny)));
            }
            format!("✏️ {} → {}", role.mention(), parts.join(" · "))
        }
        Op::DeleteOverwrite { role, .. } => format!("🗑️ {} → оверрайт снят", role.mention()),
    }
}

/// Строки плана: операции с ролями, затем по каналам с заголовком канала.
pub fn plan_lines(plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current: Option<ChannelId> = None;
    let mut roles_header = false;
    for op in &plan.ops {
        match op.channel() {
            None if !roles_header => {
                roles_header = true;
                lines.push("**Роли сервера:**".to_string());
            }
            Some(channel) if current != Some(channel) => {
                current = Some(channel);
                lines.push(format!("**{}**", channel.mention()));
            }
            _ => {}
        }
        lines.push(op_line(op));
    }
    lines
}

/// Страница плана с кнопкой применения.
pub fn plan_view(
    command: &str,
    plan: &Plan,
    lines: &[String],
    checks: &[Check],
    requested: usize,
    notice: Option<&str>,
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let warnings = plan
        .findings
        .iter()
        .filter(|finding| finding.severity == Severity::Warning)
        .count();
    let mut rows = Vec::new();
    let (colour, description) = if plan.blocked() {
        let blockers: Vec<String> = plan
            .findings
            .iter()
            .filter(|finding| finding.severity == Severity::Blocker)
            .map(|finding| format!("⛔ {}", finding.text))
            .collect();
        (
            COLOR_BLOCKED,
            format!(
                "План нельзя применить:\n{}",
                lines_within(&blockers, DESCRIPTION_MAX - 64)
            ),
        )
    } else if plan.is_empty() {
        (
            COLOR,
            "✅ Сервер соответствует политике: изменений нет.".to_string(),
        )
    } else {
        let page = Page::of(lines.len(), PER_PAGE, requested);
        let shown: Vec<String> = page
            .slice(lines, PER_PAGE)
            .iter()
            .map(|line| truncate(line, LINE_MAX))
            .collect();
        rows.extend(page.buttons(command, ACTION_PLAN, &[]));
        let fingerprint = plan.fingerprint();
        let label = format!("Применить ({})", plan.ops.len());
        let args: [&dyn Display; 1] = [&fingerprint];
        rows.push(confirm_row(
            command,
            (ACTION_APPLY, &args[..], label.as_str()),
            ACTION_CANCEL,
        ));
        (
            if warnings > 0 { COLOR_WARNING } else { COLOR },
            lines_within(&shown, DESCRIPTION_MAX),
        )
    };
    let mut description = description;
    if let Some(notice) = notice {
        description = format!("{notice}\n\n{description}");
    }
    rows.push(nav_row(command, &[Nav::Setup, Nav::Audit]));

    let embed = CreateEmbed::new()
        .colour(colour)
        .title("🔄 План изменений")
        .description(description)
        .footer(CreateEmbedFooter::new(format!(
            "Операций: {} · Предупреждений: {warnings} · Подробности — в аудите",
            plan.ops.len()
        )));
    let embed = if checks.is_empty() {
        embed
    } else {
        embed.field(
            "Проверка модели после применения",
            truncate(&check_lines(checks).join("\n"), 1024),
            false,
        )
    };
    (embed, rows)
}

/// Строки проверок: пройдена или нет, и где именно нарушена.
pub fn check_lines(checks: &[Check]) -> Vec<String> {
    checks
        .iter()
        .map(|check| {
            if check.ok() {
                format!("✅ {}", check.title)
            } else {
                let mentions: Vec<String> = check
                    .failing
                    .iter()
                    .map(|channel| channel.mention().to_string())
                    .collect();
                format!(
                    "❌ {} — {}",
                    check.title,
                    crate::domain::text::join_within(&mentions, 160)
                )
            }
        })
        .collect()
}

/// Справочник прав Discord по разделам: название, уровень, где действует (T V S или сервер).
pub fn catalog_lines() -> Vec<String> {
    use crate::domain::permissions::{CATALOG, Category};

    let mut lines = Vec::new();
    for category in Category::ALL {
        lines.push(format!("**{}**", category.label()));
        lines.extend(
            CATALOG
                .iter()
                .filter(|spec| spec.category == category)
                .map(|spec| {
                    format!(
                        "• {} (`{}`) — {} · {}",
                        spec.name,
                        spec.key,
                        spec.grade.label().to_lowercase(),
                        spec.scope.label()
                    )
                }),
        );
    }
    lines
}

/// Страница справочника прав.
pub fn catalog(
    command: &str,
    lines: &[String],
    requested: usize,
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let page = Page::of(lines.len(), PER_PAGE, requested);
    let embed = CreateEmbed::new()
        .colour(COLOR)
        .title("📖 Права Discord")
        .description(lines_within(page.slice(lines, PER_PAGE), DESCRIPTION_MAX))
        .footer(CreateEmbedFooter::new(format!(
            "Уровни: обычные → доверенные → модерация → управление → администратор. \
             T — текст, V — голос, S — трибуна. {}",
            page.label()
        )));
    let mut rows = Vec::new();
    rows.extend(page.buttons(command, ACTION_CATALOG, &[]));
    rows.push(nav_row(command, &[Nav::Setup, Nav::Audit]));
    (embed, rows)
}

pub fn progress(done: usize, total: usize) -> CreateEmbed {
    CreateEmbed::new()
        .colour(COLOR)
        .title("⏳ Применение политики…")
        .description(format!("Выполнено {done} из {total} операций."))
}

/// Итог применения; `failure` — операция, на которой остановились, и текст ошибки.
pub fn applied(done: usize, total: usize, failure: Option<(&Op, &str)>) -> CreateEmbed {
    match failure {
        None => CreateEmbed::new()
            .colour(COLOR)
            .title("✅ Политика применена")
            .description(format!(
                "Выполнено операций: {done}. Повторный запуск «План изменений» покажет пустой \
                 план — это проверка, что всё на месте."
            )),
        Some((op, error)) => CreateEmbed::new()
            .colour(COLOR_BLOCKED)
            .title("⚠️ Применение остановлено")
            .description(format!(
                "Выполнено {done} из {total}. Ошибка на операции:\n{}\n\n{error}\n\nОперации \
                 идемпотентны: устраните причину и запустите «План изменений» снова — выполненное \
                 повторяться не будет.",
                truncate(&op_line(op), LINE_MAX)
            )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;
    use crate::domain::policy::Finding;

    fn description(embed: CreateEmbed) -> String {
        let json: Value = serde_json::to_value(embed).unwrap();
        json["description"].as_str().unwrap_or_default().to_string()
    }

    #[test]
    fn op_lines_name_permissions_in_russian() {
        let op = Op::SetOverwrite {
            channel: ChannelId::new(5),
            role: RoleId::new(7),
            before: None,
            allow: Permissions::VIEW_CHANNEL,
            deny: Permissions::SEND_MESSAGES,
        };
        assert_eq!(
            op_line(&op),
            "✏️ <@&7> → ✅ «Просматривать каналы» · ⛔ «Отправлять сообщения»"
        );
        let edit = Op::EditRole {
            role: RoleId::new(1),
            before: Permissions::ADMINISTRATOR,
            after: Permissions::SPEAK,
        };
        assert_eq!(
            op_line(&edit),
            "🛡️ <@&1>: снять «Администратор»; выдать «Говорить»"
        );
    }

    #[test]
    fn plan_lines_group_by_channel() {
        let plan = Plan {
            ops: vec![
                Op::EditRole {
                    role: RoleId::new(1),
                    before: Permissions::empty(),
                    after: Permissions::SPEAK,
                },
                Op::DeleteOverwrite {
                    channel: ChannelId::new(5),
                    role: RoleId::new(2),
                    before: (Permissions::empty(), Permissions::empty()),
                },
                Op::DeleteOverwrite {
                    channel: ChannelId::new(5),
                    role: RoleId::new(3),
                    before: (Permissions::empty(), Permissions::empty()),
                },
            ],
            ..Plan::default()
        };
        let lines = plan_lines(&plan);
        assert_eq!(lines[0], "**Роли сервера:**");
        assert_eq!(lines[2], "**<#5>**");
        assert_eq!(lines.len(), 5, "заголовок канала один на канал");
    }

    #[test]
    fn plan_view_fits_limits_and_offers_apply_only_when_possible() {
        let many: Vec<Op> = (0..300)
            .map(|i| Op::SetOverwrite {
                channel: ChannelId::new(i / 3 + 1),
                role: RoleId::new(i + 1),
                before: None,
                allow: Permissions::empty(),
                deny: Permissions::all(),
            })
            .collect();
        let plan = Plan {
            ops: many,
            ..Plan::default()
        };
        let lines = plan_lines(&plan);
        let checks = [
            Check {
                title: "Без ролей не виден ни один канал",
                failing: vec![],
            },
            Check {
                title: "Чат-мут запрещает писать",
                failing: (1..200).map(ChannelId::new).collect(),
            },
        ];
        for page in [0, 3, 10_000] {
            let (embed, rows) = plan_view("perms", &plan, &lines, &checks, page, None);
            let json: Value = serde_json::to_value(embed).unwrap();
            assert!(json["description"].as_str().unwrap().chars().count() <= 4096);
            let field = json["fields"][0]["value"].as_str().unwrap();
            assert!(field.chars().count() <= 1024);
            assert!(field.contains("✅") && field.contains("❌"));
            assert_eq!(rows.len(), 3, "страницы, подтверждение, навигация");
        }

        let blocked = Plan {
            findings: vec![Finding {
                severity: Severity::Blocker,
                text: "нет прав".into(),
            }],
            ..Plan::default()
        };
        let (embed, rows) = plan_view("perms", &blocked, &[], &[], 0, Some("Состояние изменилось"));
        let text = description(embed);
        assert!(text.starts_with("Состояние изменилось"));
        assert!(text.contains("нельзя применить"));
        assert_eq!(rows.len(), 1, "только навигация");

        let (embed, rows) = plan_view("perms", &Plan::default(), &[], &[], 0, None);
        assert!(description(embed).contains("соответствует"));
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn setup_panel_has_four_selects_and_navigation() {
        let (_, rows) = setup("perms", None);
        assert_eq!(rows.len(), 5);
        let json: Value = serde_json::to_value(&rows[0]).unwrap();
        assert_eq!(json["components"][0]["custom_id"], "perms:role:member");
        assert_eq!(json["components"][0]["min_values"], 1);
        let json: Value = serde_json::to_value(&rows[1]).unwrap();
        assert_eq!(json["components"][0]["min_values"], 0);

        let mut policy = Policy::new(RoleId::new(10));
        policy.chat_mute = Some(RoleId::new(12));
        let (embed, _) = setup("perms", Some(&policy));
        let text = description(embed);
        assert!(text.contains("<@&10>"));
        assert!(text.contains("<@&12>"));
        assert!(!text.contains("не настроена"));
    }

    #[test]
    fn catalog_lists_every_permission_once() {
        use crate::domain::permissions::CATALOG;
        let lines = catalog_lines();
        let entries = lines.iter().filter(|line| line.starts_with("• ")).count();
        assert_eq!(entries, CATALOG.len());
        for spec in CATALOG {
            assert!(
                lines.iter().any(|line| line.contains(spec.name)),
                "{}",
                spec.name
            );
        }
        for page in [0, 1, 99] {
            let (embed, rows) = catalog("perms", &lines, page);
            assert!(description(embed).chars().count() <= 4096);
            assert_eq!(rows.len(), 2);
        }
    }

    #[test]
    fn applied_reports_failure_point() {
        let op = Op::DeleteOverwrite {
            channel: ChannelId::new(5),
            role: RoleId::new(2),
            before: (Permissions::empty(), Permissions::empty()),
        };
        let text = description(applied(3, 10, Some((&op, "нет прав"))));
        assert!(text.contains("3 из 10"));
        assert!(text.contains("<@&2>"));
        assert!(description(applied(10, 10, None)).contains("10"));
        let _ = BTreeSet::<RoleId>::new();
    }
}
