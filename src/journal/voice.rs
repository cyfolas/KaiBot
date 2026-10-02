//! Журнал голосовых каналов: входы, выходы и переходы. Чистая логика — покрыта тестами.
//!
//! Discord присылает только новое состояние участника; прежнее берётся из кэша serenity, который
//! получает голосовые состояния вместе с сервером при подключении. Изменения микрофона, звука,
//! камеры и трансляции — не переходы и в журнал не попадают.

use serenity::all::*;

const COLOR_JOINED: Colour = Colour(0x0057_F287);
const COLOR_LEFT: Colour = Colour(0x00ED_4245);
const COLOR_MOVED: Colour = Colour(0x0058_65F2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Joined(ChannelId),
    Left(ChannelId),
    Moved { from: ChannelId, to: ChannelId },
}

/// Переход между каналами по прежнему и новому каналу участника.
pub fn transition(old: Option<ChannelId>, new: Option<ChannelId>) -> Option<Transition> {
    match (old, new) {
        (None, Some(to)) => Some(Transition::Joined(to)),
        (Some(from), None) => Some(Transition::Left(from)),
        (Some(from), Some(to)) if from != to => Some(Transition::Moved { from, to }),
        _ => None,
    }
}

pub fn entry(user: UserId, name: &str, transition: Transition) -> CreateEmbed {
    let (colour, text) = match transition {
        Transition::Joined(to) => (
            COLOR_JOINED,
            format!("🔊 {} — вход в {}", user.mention(), to.mention()),
        ),
        Transition::Left(from) => (
            COLOR_LEFT,
            format!("🔇 {} — выход из {}", user.mention(), from.mention()),
        ),
        Transition::Moved { from, to } => (
            COLOR_MOVED,
            format!(
                "🔀 {} — переход из {} в {}",
                user.mention(),
                from.mention(),
                to.mention()
            ),
        ),
    };
    CreateEmbed::new()
        .colour(colour)
        .author(CreateEmbedAuthor::new(name))
        .description(text)
        .footer(CreateEmbedFooter::new(format!("Участник: {user}")))
        .timestamp(Timestamp::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_truth_table() {
        let a = Some(ChannelId::new(1));
        let b = Some(ChannelId::new(2));
        assert_eq!(
            transition(None, a),
            Some(Transition::Joined(ChannelId::new(1)))
        );
        assert_eq!(
            transition(a, None),
            Some(Transition::Left(ChannelId::new(1)))
        );
        assert_eq!(
            transition(a, b),
            Some(Transition::Moved {
                from: ChannelId::new(1),
                to: ChannelId::new(2)
            })
        );
        // Мьют, камера, трансляция — тот же канал.
        assert_eq!(transition(a, a), None);
        assert_eq!(transition(None, None), None);
    }
}
