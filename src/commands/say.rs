//! `/say` — сообщение от имени бота.
//!
//! С опцией `text` сообщение отправляется сразу (однострочное: Discord не допускает переносов
//! в опциях). Без неё открывается форма для многострочного текста.
//!
//! Бот не становится «доверенным посредником»: отправить можно только туда, где автор сам может
//! писать, а уведомления от упоминаний ограничены правами автора (см. [`mentions`]).

mod mentions;

use serenity::all::*;
use tracing::info;

use crate::error::{AppError, Result};
use crate::framework::{CustomId, Cx, Options, SlashCommand, ephemeral, modal_value};

const NAME: &str = "say";

/// Лимит длины обычного сообщения Discord.
const MAX_LEN: u16 = 2000;

const ACTION_MODAL: &str = "modal";
const INPUT_TEXT: &str = "text";

pub struct Say;

#[async_trait]
impl SlashCommand for Say {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Отправить сообщение от имени бота"
    }

    /// Говорить от имени бота — административное действие.
    fn permission(&self) -> Permissions {
        Permissions::MANAGE_GUILD
    }

    fn options(&self) -> Vec<CreateCommandOption> {
        vec![
            CreateCommandOption::new(
                CommandOptionType::String,
                "text",
                "Текст в одну строку; без него откроется форма для многострочного текста",
            )
            .max_length(MAX_LEN),
            CreateCommandOption::new(
                CommandOptionType::Channel,
                "channel",
                "Канал (по умолчанию — текущий)",
            )
            .channel_types(vec![ChannelType::Text, ChannelType::News]),
            CreateCommandOption::new(
                CommandOptionType::Boolean,
                "mentions",
                "Уведомлять упомянутых (по умолчанию упоминания не присылают уведомлений)",
            ),
        ]
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        let options = Options::of(command);
        let channel = options.channel("channel").unwrap_or(command.channel_id);
        let notify = options.bool("mentions").unwrap_or(false);

        let response = match options.str("text") {
            Some(text) => {
                let link = send(cx, channel, text, notify).await?;
                ephemeral(format!("✅ Отправлено: {link}"))
            }
            None => {
                // Права проверяются до формы, чтобы отказ не стоил набранного текста.
                cx.require_post(channel, Permissions::empty())?;
                CreateInteractionResponse::Modal(text_modal(channel, notify))
            }
        };
        command.create_response(cx, response).await?;
        Ok(())
    }

    async fn modal(&self, cx: Cx<'_>, modal: &ModalInteraction) -> Result<()> {
        let mut id = CustomId::parse(&modal.data.custom_id);
        if id.action != ACTION_MODAL {
            return Err(AppError::stale());
        }
        let channel = id.arg::<ChannelId>()?;
        let notify = id.arg::<bool>()?;

        let text = modal_value(modal, INPUT_TEXT).unwrap_or_default();
        let link = send(cx, channel, text, notify).await?;
        modal
            .create_response(cx, ephemeral(format!("✅ Отправлено: {link}")))
            .await?;
        Ok(())
    }
}

fn text_modal(channel: ChannelId, notify: bool) -> CreateModal {
    let input = CreateInputText::new(InputTextStyle::Paragraph, "Текст сообщения", INPUT_TEXT)
        .placeholder("Поддерживается Markdown Discord")
        .max_length(MAX_LEN)
        .required(true);

    CreateModal::new(
        CustomId::encode(NAME, ACTION_MODAL, &[&channel, &notify]),
        "Сообщение от имени бота",
    )
    .components(vec![CreateActionRow::InputText(input)])
}

/// Отправляет сообщение и возвращает ссылку на него.
///
/// Права автора в целевом канале проверяются здесь, непосредственно перед отправкой: между
/// открытием формы и её отправкой они могли измениться.
async fn send(cx: Cx<'_>, channel: ChannelId, text: &str, notify: bool) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::user("Сообщение не может быть пустым."));
    }

    let granted = cx.require_post(channel, Permissions::empty())?;
    let allowed = if notify {
        // Кэш сервера нельзя держать через `await`, поэтому упоминания вычисляются заранее.
        let guild = cx.guild()?;
        mentions::allowed(text, granted, |role| {
            guild.roles.get(&role).is_some_and(|role| role.mentionable)
        })
    } else {
        mentions::silent()
    };

    let message = channel
        .send_message(
            cx,
            CreateMessage::new().content(text).allowed_mentions(allowed),
        )
        .await?;

    // Журнал аудита: в Discord автором сообщения будет бот, реальный автор виден только здесь.
    let author = &cx.caller.member.user;
    info!(
        target: "audit",
        "/say: {} ({}) → канал {}, сообщение {}",
        author.tag(),
        author.id,
        channel,
        message.id
    );
    Ok(message.link())
}
