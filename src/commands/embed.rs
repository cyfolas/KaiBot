//! `/embed` — конструктор embed-сообщений: опции → форма → предпросмотр → публикация.
//!
//! Между шагами состояние передаётся в `custom_id` (канал, цвет, метка времени), а сам embed —
//! в эфемерном сообщении предпросмотра, поэтому бот ничего не хранит.

mod color;
mod draft;

// `Embed` здесь — сама команда, поэтому тип serenity переименован.
use serenity::all::{Embed as MessageEmbed, *};
use tracing::info;

use self::draft::EmbedDraft;
use crate::error::{AppError, Result};
use crate::framework::{CustomId, Cx, Options, SlashCommand, modal_value};

const NAME: &str = "embed";

/// Права автора в целевом канале сверх просмотра и отправки.
const POST_EXTRA: Permissions = Permissions::EMBED_LINKS;

const ACTION_MODAL: &str = "modal";
const ACTION_PUBLISH: &str = "publish";
const ACTION_CANCEL: &str = "cancel";

const INPUT_TITLE: &str = "title";
const INPUT_DESCRIPTION: &str = "description";
const INPUT_IMAGE: &str = "image";
const INPUT_THUMBNAIL: &str = "thumbnail";
const INPUT_FOOTER: &str = "footer";

/// Максимум для поля формы Discord (меньше лимита описания embed).
const INPUT_MAX: u16 = 4000;
const URL_INPUT_MAX: u16 = 1000;

pub struct Embed;

#[async_trait]
impl SlashCommand for Embed {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Конструктор embed-сообщений с предпросмотром"
    }

    /// Публиковать от имени бота — административное действие.
    fn permission(&self) -> Permissions {
        Permissions::MANAGE_GUILD
    }

    fn options(&self) -> Vec<CreateCommandOption> {
        vec![
            CreateCommandOption::new(
                CommandOptionType::Channel,
                "channel",
                "Канал (по умолчанию — текущий)",
            )
            .channel_types(vec![ChannelType::Text, ChannelType::News]),
            CreateCommandOption::new(
                CommandOptionType::String,
                "color",
                "Цвет: #5865F2, #F00 или название (red, green, yellow, blurple…)",
            )
            .max_length(16),
            CreateCommandOption::new(
                CommandOptionType::Boolean,
                "timestamp",
                "Показать время публикации",
            ),
        ]
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        let options = Options::of(command);
        let channel = options.channel("channel").unwrap_or(command.channel_id);
        // Права и цвет проверяются до открытия формы, чтобы ошибка не стоила набранного текста.
        cx.require_post(channel, POST_EXTRA)?;
        let color = options
            .str("color")
            .map(color::parse)
            .transpose()?
            .unwrap_or(color::DEFAULT);
        let timestamp = options.bool("timestamp").unwrap_or(false);

        command
            .create_response(
                cx,
                CreateInteractionResponse::Modal(form(channel, color, timestamp)),
            )
            .await?;
        Ok(())
    }

    async fn modal(&self, cx: Cx<'_>, modal: &ModalInteraction) -> Result<()> {
        let mut id = CustomId::parse(&modal.data.custom_id);
        if id.action != ACTION_MODAL {
            return Err(AppError::stale());
        }
        let channel = id.arg::<ChannelId>()?;
        let color = Colour::new(id.arg::<u32>()?);
        let timestamp = id.arg::<bool>()?;
        cx.require_post(channel, POST_EXTRA)?;

        let embed = EmbedDraft {
            title: modal_value(modal, INPUT_TITLE),
            description: modal_value(modal, INPUT_DESCRIPTION),
            image_url: modal_value(modal, INPUT_IMAGE),
            thumbnail_url: modal_value(modal, INPUT_THUMBNAIL),
            footer: modal_value(modal, INPUT_FOOTER),
            color,
            timestamp,
        }
        .build()?;

        let buttons = vec![
            CreateButton::new(CustomId::encode(NAME, ACTION_PUBLISH, &[&channel]))
                .label("Опубликовать")
                .style(ButtonStyle::Success),
            CreateButton::new(CustomId::encode(NAME, ACTION_CANCEL, &[]))
                .label("Отмена")
                .style(ButtonStyle::Secondary),
        ];
        let preview = CreateInteractionResponseMessage::new()
            .content(format!(
                "Предпросмотр. Сообщение будет опубликовано в {}.",
                channel.mention()
            ))
            .embed(embed)
            .components(vec![CreateActionRow::Buttons(buttons)])
            .ephemeral(true);

        modal
            .create_response(cx, CreateInteractionResponse::Message(preview))
            .await?;
        Ok(())
    }

    async fn component(&self, cx: Cx<'_>, component: &ComponentInteraction) -> Result<()> {
        let mut id = CustomId::parse(&component.data.custom_id);
        let outcome = match id.action {
            ACTION_PUBLISH => {
                let channel = id.arg::<ChannelId>()?;
                cx.require_post(channel, POST_EXTRA)?;

                let preview = component.message.embeds.first().cloned().ok_or_else(|| {
                    AppError::user("Предпросмотр не найден — вызовите /embed заново.")
                })?;
                let link = publish(cx, channel, preview).await?;
                format!("✅ Опубликовано: {link}")
            }
            ACTION_CANCEL => "Публикация отменена.".to_string(),
            _ => return Err(AppError::stale()),
        };

        // Предпросмотр заменяется итогом, чтобы кнопку нельзя было нажать повторно.
        let update = CreateInteractionResponseMessage::new()
            .content(outcome)
            .embeds(vec![])
            .components(vec![]);
        component
            .create_response(cx, CreateInteractionResponse::UpdateMessage(update))
            .await?;
        Ok(())
    }
}

fn form(channel: ChannelId, color: Colour, timestamp: bool) -> CreateModal {
    let input = |style, label: &str, id: &str, max: u16| {
        CreateInputText::new(style, label, id)
            .max_length(max)
            .required(false)
    };

    let title = input(
        InputTextStyle::Short,
        "Заголовок",
        INPUT_TITLE,
        draft::TITLE_MAX,
    );
    let description = input(
        InputTextStyle::Paragraph,
        "Текст (Markdown)",
        INPUT_DESCRIPTION,
        INPUT_MAX,
    );
    let image = input(
        InputTextStyle::Short,
        "Изображение-баннер (ссылка)",
        INPUT_IMAGE,
        URL_INPUT_MAX,
    )
    .placeholder("https://…/banner.png");
    let thumbnail = input(
        InputTextStyle::Short,
        "Миниатюра справа (ссылка)",
        INPUT_THUMBNAIL,
        URL_INPUT_MAX,
    )
    .placeholder("https://…/icon.png");
    let footer = input(
        InputTextStyle::Short,
        "Подпись внизу",
        INPUT_FOOTER,
        draft::FOOTER_MAX,
    );

    let custom_id = CustomId::encode(NAME, ACTION_MODAL, &[&channel, &color.0, &timestamp]);
    CreateModal::new(custom_id, "Новое embed-сообщение").components(
        [title, description, image, thumbnail, footer]
            .into_iter()
            .map(CreateActionRow::InputText)
            .collect(),
    )
}

async fn publish(cx: Cx<'_>, channel: ChannelId, preview: MessageEmbed) -> Result<String> {
    let has_timestamp = preview.timestamp.is_some();
    let mut embed = CreateEmbed::from(preview);
    // Время предпросмотра заменяется моментом публикации.
    if has_timestamp {
        embed = embed.timestamp(Timestamp::now());
    }

    let message = channel
        .send_message(cx, CreateMessage::new().embed(embed))
        .await?;

    let author = &cx.caller.member.user;
    info!(
        target: "audit",
        "/embed: {} ({}) → канал {}, сообщение {}",
        author.tag(),
        author.id,
        channel,
        message.id
    );
    Ok(message.link())
}
