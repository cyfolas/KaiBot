//! Ответы на взаимодействия.

use serenity::all::*;
use tracing::warn;

use crate::error::{AppError, Chain};

/// Ответ-сообщение, видимое только вызвавшему.
pub fn ephemeral(content: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .content(content)
            .ephemeral(true),
    )
}

/// Сообщает пользователю об ошибке.
///
/// Обработчики отвечают на взаимодействие последним действием либо сначала откладывают ответ
/// (`defer`). Поэтому если исходный ответ уже занят, это заглушка «бот думает…», и её нужно
/// заменить текстом ошибки.
pub(super) async fn report_error(ctx: &Context, id: InteractionId, token: &str, err: &AppError) {
    let content = format!("❌ {}", err.user_message());

    if ephemeral(&content).execute(ctx, (id, token)).await.is_ok() {
        return;
    }

    let edit = EditInteractionResponse::new()
        .content(content)
        .embeds(vec![])
        .components(vec![]);
    if let Err(e) = edit.execute(ctx, token).await {
        warn!("Не удалось сообщить пользователю об ошибке: {}", Chain(&e));
    }
}
