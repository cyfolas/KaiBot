//! Интерактивные панели: постраничный вывод, подтверждение, меню выбора ролей.
//!
//! Панель — эфемерное сообщение с embed и компонентами, которое команда заменяет целиком на
//! каждом шаге (`UpdateMessage`). Состояние шага передаётся в `custom_id`, поэтому бот ничего не
//! хранит, а любая кнопка переживает перезапуск. Discord допускает до 5 рядов компонентов,
//! до 5 кнопок в ряду и одно меню на ряд.

use std::fmt::Display;

use serenity::all::*;

use super::CustomId;

/// Страница списка.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// Номер с нуля, не больше `count - 1`.
    pub index: usize,
    /// Число страниц, не меньше одной.
    pub count: usize,
}

impl Page {
    /// Страница `requested` списка из `total` элементов по `per_page`; выход за край
    /// прижимается к последней странице.
    pub fn of(total: usize, per_page: usize, requested: usize) -> Self {
        let count = total.div_ceil(per_page.max(1)).max(1);
        Self {
            index: requested.min(count - 1),
            count,
        }
    }

    /// Элементы этой страницы.
    pub fn slice<T>(self, items: &[T], per_page: usize) -> &[T] {
        let start = (self.index * per_page).min(items.len());
        let end = (start + per_page).min(items.len());
        &items[start..end]
    }

    pub fn label(self) -> String {
        format!("Страница {}/{}", self.index + 1, self.count)
    }

    /// Ряд навигации «◀ · n/N · ▶»; `None`, если страница одна. Каждая кнопка несёт
    /// `<команда>:<действие>:<аргументы…>:<номер страницы>`.
    pub fn buttons(
        self,
        command: &str,
        action: &str,
        args: &[&dyn Display],
    ) -> Option<CreateActionRow> {
        if self.count <= 1 {
            return None;
        }
        let target = |page: usize| {
            let mut all: Vec<&dyn Display> = args.to_vec();
            all.push(&page);
            CustomId::encode(command, action, &all)
        };
        let previous = self.index.saturating_sub(1);
        let next = (self.index + 1).min(self.count - 1);
        Some(CreateActionRow::Buttons(vec![
            CreateButton::new(target(previous))
                .emoji(emoji("◀️"))
                .style(ButtonStyle::Secondary)
                .disabled(self.index == 0),
            CreateButton::new(CustomId::encode(command, "noop", &[&self.index]))
                .label(format!("{}/{}", self.index + 1, self.count))
                .style(ButtonStyle::Secondary)
                .disabled(true),
            CreateButton::new(target(next))
                .emoji(emoji("▶️"))
                .style(ButtonStyle::Secondary)
                .disabled(self.index + 1 >= self.count),
        ]))
    }
}

/// Ряд «подтвердить / отмена».
pub fn confirm_row(
    command: &str,
    confirm: (&str, &[&dyn Display], &str),
    cancel_action: &str,
) -> CreateActionRow {
    let (action, args, label) = confirm;
    CreateActionRow::Buttons(vec![
        CreateButton::new(CustomId::encode(command, action, args))
            .label(label)
            .style(ButtonStyle::Success),
        CreateButton::new(CustomId::encode(command, cancel_action, &[]))
            .label("Отмена")
            .style(ButtonStyle::Secondary),
    ])
}

/// Меню выбора ролей сервера. `min = 0` позволяет снять выбор.
pub fn role_select(
    custom_id: String,
    placeholder: &str,
    defaults: &[RoleId],
    min: u8,
    max: u8,
) -> CreateActionRow {
    // Discord требует, чтобы число значений по умолчанию попадало в [min, max]; пустой список
    // при `min = 1` отклоняется, поэтому без выбора поле не передаётся вовсе.
    let default_roles = (!defaults.is_empty()).then(|| defaults.to_vec());
    CreateActionRow::SelectMenu(
        CreateSelectMenu::new(custom_id, CreateSelectMenuKind::Role { default_roles })
            .placeholder(placeholder)
            .min_values(min)
            .max_values(max),
    )
}

/// Эмодзи кнопки из строки Unicode (с селектором варианта, как присылает клиент Discord).
pub fn emoji(unicode: &str) -> ReactionType {
    ReactionType::Unicode(unicode.to_string())
}

/// Эфемерное сообщение-панель: embed и ряды компонентов.
pub fn panel(embed: CreateEmbed, rows: Vec<CreateActionRow>) -> CreateInteractionResponseMessage {
    CreateInteractionResponseMessage::new()
        .embed(embed)
        .components(rows)
        .ephemeral(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_clamp_and_slice() {
        let items: Vec<u32> = (0..23).collect();
        let page = Page::of(items.len(), 10, 7);
        assert_eq!(page, Page { index: 2, count: 3 });
        assert_eq!(page.slice(&items, 10), &[20, 21, 22]);
        assert_eq!(Page::of(0, 10, 0), Page { index: 0, count: 1 });
        assert!(Page::of(0, 10, 0).slice(&items[..0], 10).is_empty());
        assert_eq!(Page::of(10, 10, 0).count, 1);
    }

    #[test]
    fn role_select_omits_empty_defaults() {
        let json = serde_json::to_value(role_select("x".into(), "p", &[], 1, 1)).unwrap();
        assert!(json["components"][0].get("default_values").is_none());
        let json =
            serde_json::to_value(role_select("x".into(), "p", &[RoleId::new(5)], 1, 1)).unwrap();
        let defaults = json["components"][0]["default_values"].as_array().unwrap();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0]["type"], "role");
    }

    #[test]
    fn navigation_encodes_page_in_custom_id() {
        assert!(Page::of(5, 10, 0).buttons("perms", "audit", &[]).is_none());
        let row = Page::of(25, 10, 1)
            .buttons("perms", "audit", &[&"x"])
            .unwrap();
        let json = serde_json::to_value(row).unwrap();
        let ids: Vec<&str> = json["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|button| button["custom_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["perms:audit:x:0", "perms:noop:1", "perms:audit:x:2"]);
        assert_eq!(json["components"][1]["disabled"], true);
    }
}
