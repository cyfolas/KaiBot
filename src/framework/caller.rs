//! Модель доступа.
//!
//! Два независимых уровня:
//! * **Права на сервере** берутся только из Discord: права ролей, оверрайты каналов, владение
//!   сервером, тайм-аут. Бот не хранит ID ролей и не дублирует иерархию — её задаёт сервер, и
//!   любые изменения ролей действуют сразу.
//! * **Разработчики бота** — уровень приложения (`AppState::developers`), с сервером не связан и
//!   прав на нём не даёт.

use serenity::all::*;

use crate::error::{AppError, Result};

/// Участник сервера, вызвавший взаимодействие.
#[derive(Clone, Copy)]
pub struct Caller<'a> {
    pub guild_id: GuildId,
    pub member: &'a Member,
    /// Канал взаимодействия.
    pub channel_id: ChannelId,
    in_thread: bool,
}

impl<'a> Caller<'a> {
    pub fn new(
        guild_id: Option<GuildId>,
        member: Option<&'a Member>,
        channel_id: ChannelId,
        channel: Option<&PartialChannel>,
    ) -> Result<Self> {
        match (guild_id, member) {
            (Some(guild_id), Some(member)) => Ok(Self {
                guild_id,
                member,
                channel_id,
                in_thread: channel.is_some_and(|channel| is_thread(channel.kind)),
            }),
            _ => Err(AppError::user("Команды бота работают только на серверах.")),
        }
    }

    /// Права в канале взаимодействия. Discord присылает их уже вычисленными — с учётом ролей,
    /// оверрайтов канала, владения сервером и тайм-аута.
    pub fn permissions(&self) -> Permissions {
        self.member.permissions.unwrap_or_else(Permissions::empty)
    }

    pub fn require(&self, required: Permissions) -> Result<()> {
        let missing = required - self.permissions();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(AppError::user(format!(
                "Не хватает прав: {}.",
                describe(missing)
            )))
        }
    }

    /// Сервер из кэша. Отсутствует только в первые секунды после подключения к Gateway.
    pub(super) fn guild<'c>(&self, cache: &'c Cache) -> Result<GuildRef<'c>> {
        cache.guild(self.guild_id).ok_or_else(|| {
            AppError::user("Данные сервера ещё загружаются, повторите через несколько секунд.")
        })
    }

    /// Требует, чтобы участник сам мог опубликовать сообщение в `target`, и возвращает его права
    /// там. `extra` — права сверх просмотра и отправки (например, «Встраивать ссылки»).
    ///
    /// Бот не должен быть посредником, расширяющим чужие права: от его имени можно писать
    /// только туда, куда автор может писать сам.
    pub(super) fn require_post(
        &self,
        cache: &Cache,
        target: ChannelId,
        extra: Permissions,
    ) -> Result<Permissions> {
        let (granted, thread) = self.permissions_in(cache, target)?;
        let send = if thread {
            Permissions::SEND_MESSAGES_IN_THREADS
        } else {
            Permissions::SEND_MESSAGES
        };

        let missing = (Permissions::VIEW_CHANNEL | send | extra) - granted;
        if missing.is_empty() {
            Ok(granted)
        } else {
            Err(AppError::user(format!(
                "У вас нет прав в {}: {}. Бот публикует только туда, куда вы можете сами.",
                target.mention(),
                describe(missing)
            )))
        }
    }

    /// Права в канале `target` того же сервера и признак ветки.
    ///
    /// Для канала взаимодействия используются права от Discord — это точно и работает в ветках.
    /// Для другого канала права считаются по кэшу (роли + оверрайты); кэш не учитывает тайм-аут,
    /// поэтому он применяется здесь. Выбор канала в опциях ограничен обычными каналами, так что
    /// чужой канал не бывает веткой.
    fn permissions_in(&self, cache: &Cache, target: ChannelId) -> Result<(Permissions, bool)> {
        if target == self.channel_id {
            return Ok((self.permissions(), self.in_thread));
        }

        let guild = self.guild(cache)?;
        let channel = guild
            .channels
            .get(&target)
            .ok_or_else(|| AppError::user("Канал не найден на этом сервере."))?;

        let mut granted = guild.user_permissions_in(channel, self.member);
        if self.is_timed_out() && !granted.administrator() {
            granted &= Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY;
        }
        Ok((granted, false))
    }

    fn is_timed_out(&self) -> bool {
        self.member
            .communication_disabled_until
            .is_some_and(|until| until > Timestamp::now())
    }
}

fn is_thread(kind: ChannelType) -> bool {
    matches!(
        kind,
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
    )
}

/// Названия прав, как в русском клиенте Discord. Для прочих — английские названия serenity.
const NAMES: &[(Permissions, &str)] = &[
    (Permissions::ADMINISTRATOR, "Администратор"),
    (Permissions::MANAGE_GUILD, "Управлять сервером"),
    (Permissions::MANAGE_ROLES, "Управлять ролями"),
    (Permissions::MANAGE_CHANNELS, "Управлять каналами"),
    (Permissions::MANAGE_MESSAGES, "Управлять сообщениями"),
    (Permissions::VIEW_CHANNEL, "Просматривать каналы"),
    (Permissions::SEND_MESSAGES, "Отправлять сообщения"),
    (
        Permissions::SEND_MESSAGES_IN_THREADS,
        "Отправлять сообщения в ветках",
    ),
    (Permissions::EMBED_LINKS, "Встраивать ссылки"),
    (
        Permissions::MENTION_EVERYONE,
        "Упоминание @everyone, @here и всех ролей",
    ),
];

/// «Управлять сервером», «Встраивать ссылки» — в порядке битов.
fn describe(permissions: Permissions) -> String {
    permissions
        .iter()
        .map(
            |flag| match NAMES.iter().find(|(known, _)| *known == flag) {
                Some((_, name)) => format!("«{name}»"),
                None => format!("«{}»", flag.get_permission_names().join(", ")),
            },
        )
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_permissions_in_russian() {
        assert_eq!(
            describe(Permissions::EMBED_LINKS | Permissions::MANAGE_GUILD),
            "«Управлять сервером», «Встраивать ссылки»"
        );
    }

    #[test]
    fn falls_back_to_serenity_names() {
        assert_eq!(describe(Permissions::BAN_MEMBERS), "«Ban Members»");
    }
}
