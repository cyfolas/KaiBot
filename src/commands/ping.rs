//! `/ping` — состояние бота: задержка Gateway и REST API, время работы, версия.

use std::time::{Duration, Instant};

use serenity::all::*;

use crate::error::Result;
use crate::framework::{Cx, SlashCommand};

pub struct Ping;

#[async_trait]
impl SlashCommand for Ping {
    fn name(&self) -> &'static str {
        "ping"
    }

    fn description(&self) -> &'static str {
        "Состояние бота: задержка, время работы и версия"
    }

    async fn run(&self, cx: Cx<'_>, command: &CommandInteraction) -> Result<()> {
        // Время подтверждения взаимодействия — полный цикл запроса к REST API Discord.
        let started = Instant::now();
        command.defer_ephemeral(cx).await?;
        let rest = started.elapsed();

        let gateway = match cx.state.shard_latency(cx.ctx.shard_id).await {
            Some(latency) => format!("{} мс", latency.as_millis()),
            None => "измеряется…".to_string(),
        };

        let embed = CreateEmbed::new()
            .title("🏓 Pong")
            .colour(Colour::new(0x0057_F287))
            .field("Gateway", gateway, true)
            .field("REST API", format!("{} мс", rest.as_millis()), true)
            .field("Время работы", format_uptime(cx.state.uptime()), true)
            .footer(CreateEmbedFooter::new(concat!(
                "KaiBot v",
                env!("CARGO_PKG_VERSION")
            )));

        command
            .edit_response(cx, EditInteractionResponse::new().embed(embed))
            .await?;
        Ok(())
    }
}

/// Длительность в виде «2 д 3 ч 15 мин»; меньше минуты — в секундах.
fn format_uptime(uptime: Duration) -> String {
    let total = uptime.as_secs();
    if total < 60 {
        return format!("{total} с");
    }

    let (days, hours, minutes) = (total / 86_400, total / 3_600 % 24, total / 60 % 60);
    [(days, "д"), (hours, "ч"), (minutes, "мин")]
        .iter()
        .filter(|(value, _)| *value > 0)
        .map(|(value, unit)| format!("{value} {unit}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_uptime() {
        assert_eq!(format_uptime(Duration::from_secs(42)), "42 с");
        assert_eq!(format_uptime(Duration::from_secs(3_600)), "1 ч");
        assert_eq!(
            format_uptime(Duration::from_secs(2 * 86_400 + 3 * 3_600 + 15 * 60 + 7)),
            "2 д 3 ч 15 мин"
        );
    }
}
