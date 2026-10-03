//! Политика прав сервера: модель, разбор текущего состояния и план приведения к модели.
//! Чистая логика без обращений к Discord — покрыта тестами, включая идемпотентность плана и
//! инвариант безопасности мутов.
//!
//! # Модель
//!
//! Эталонная схема прав крупного сервера:
//!
//! * **@everyone не даёт ничего.** Участник без ролей не видит ни одного канала.
//! * **Роль «неверифицированный»** (если есть) тоже не даёт ничего на уровне сервера; каналы
//!   верификации открываются ей оверрайтом. После верификации её снимают и выдают роль участника.
//! * **Роль «участник»** несёт базовые права на уровне роли ([`MEMBER_BASELINE`]): просмотр
//!   каналов, сообщения, голос. Поэтому в обычных каналах оверрайтов для неё нет — **каналы
//!   нейтральны**.
//! * **Роли-ограничения** (`chat-mute`, `voice-mute`) не дают ничего на уровне роли, а в каждом
//!   канале несут запрет ([`CHAT_MUTE`], [`VOICE_MUTE`]). Запрет работает, потому что никакой
//!   другой оверрайт в канале не разрешает эти же биты: Discord складывает все разрешения ролей
//!   поверх всех запретов ролей, и одно разрешение перебило бы запрет.
//! * **Закрытые каналы** — запрет «Просматривать каналы» для @everyone и разрешение *только*
//!   «Просматривать каналы» (и «Подключаться») для ролей из списка доступа ([`GATE`]). Всё
//!   остальное роль получает на уровне сервера.
//! * **Каналы только для чтения** — запрет записи ([`WRITE`]) для @everyone и разрешение записи
//!   для авторов.
//!
//! # Инвариант безопасности мутов
//!
//! Для каждого канала `c` и каждой неадминистративной роли `r`:
//! `allow(c, r) ∩ MUTE_BITS ⊆ deny(c, @everyone)`, то есть разрешение в канале может лишь
//! снимать ограничение, наложенное на всех, но не выдавать то, что роль должна получать на
//! уровне сервера. Тогда участник с ролью мута, без персонального оверрайта и вне администрации,
//! не получает запрещённые биты: базовые права → минус запреты @everyone → минус запреты ролей
//! (в том числе мута) → плюс разрешения ролей, которые не пересекаются с `MUTE_BITS` вне
//! `deny(@everyone)`, а эти биты уже сняты. Инвариант проверяется тестом на смоделированном
//! состоянии после применения плана.
//!
//! # План
//!
//! [`plan`] сравнивает снимок сервера с моделью и выдаёт операции ([`Op`]) и замечания
//! ([`Finding`]). План **минимален** (только различия в наблюдаемых битах), **упорядочен**
//! (роли → категории → каналы) и **идемпотентен**: план по состоянию после применения пуст
//! (проверяется тестом через [`simulate`]). Доступ *людей* план не меняет, кроме того, что явно
//! требует модель; каждое такое изменение — отдельное замечание.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use serenity::all::{
    ChannelId, ChannelType, Guild, GuildChannel, Member, Mentionable, PermissionOverwriteType,
    Permissions, RoleId, UserId,
};

use super::hierarchy::{self, Hierarchy, Person};
use super::permissions::{self, OBSERVABLE, describe};
use super::roles::Tier;

/// Базовые права участника — всё обычное для текста и голоса, без уведомления всех, TTS,
/// приватных веток и приоритетного режима.
pub const MEMBER_BASELINE: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::CREATE_INSTANT_INVITE)
    .union(Permissions::CHANGE_NICKNAME)
    .union(Permissions::SEND_MESSAGES)
    .union(Permissions::SEND_MESSAGES_IN_THREADS)
    .union(Permissions::CREATE_PUBLIC_THREADS)
    .union(Permissions::EMBED_LINKS)
    .union(Permissions::ATTACH_FILES)
    .union(Permissions::ADD_REACTIONS)
    .union(Permissions::USE_EXTERNAL_EMOJIS)
    .union(Permissions::USE_EXTERNAL_STICKERS)
    .union(Permissions::READ_MESSAGE_HISTORY)
    .union(Permissions::SEND_VOICE_MESSAGES)
    .union(Permissions::SEND_POLLS)
    .union(Permissions::USE_APPLICATION_COMMANDS)
    .union(Permissions::USE_EMBEDDED_ACTIVITIES)
    .union(Permissions::USE_EXTERNAL_APPS)
    .union(Permissions::CONNECT)
    .union(Permissions::SPEAK)
    .union(Permissions::STREAM)
    .union(Permissions::USE_VAD)
    .union(Permissions::USE_SOUNDBOARD)
    .union(Permissions::USE_EXTERNAL_SOUNDS)
    .union(Permissions::REQUEST_TO_SPEAK);

/// Что запрещает чат-мут. «Использовать команды приложений» не запрещается: иначе наказанный
/// не сможет обратиться к боту (например, подать апелляцию).
pub const CHAT_MUTE: Permissions = Permissions::SEND_MESSAGES
    .union(Permissions::SEND_MESSAGES_IN_THREADS)
    .union(Permissions::CREATE_PUBLIC_THREADS)
    .union(Permissions::CREATE_PRIVATE_THREADS)
    .union(Permissions::ADD_REACTIONS)
    .union(Permissions::SEND_VOICE_MESSAGES)
    .union(Permissions::SEND_POLLS)
    .union(Permissions::SEND_TTS_MESSAGES)
    .union(Permissions::USE_EXTERNAL_APPS);

/// Что запрещает войс-мут: слушать можно, подавать голос и видео — нет.
pub const VOICE_MUTE: Permissions = Permissions::SPEAK
    .union(Permissions::STREAM)
    .union(Permissions::USE_SOUNDBOARD)
    .union(Permissions::USE_EXTERNAL_SOUNDS)
    .union(Permissions::PRIORITY_SPEAKER)
    .union(Permissions::REQUEST_TO_SPEAK)
    .union(Permissions::USE_EMBEDDED_ACTIVITIES)
    .union(Permissions::SET_VOICE_CHANNEL_STATUS);

/// Биты, которые роли-ограничения запрещают в каналах.
pub const MUTE_BITS: Permissions = CHAT_MUTE.union(VOICE_MUTE);

/// «Ворота» закрытого канала: единственное, что разрешается ролям из списка доступа.
pub const GATE: Permissions = Permissions::VIEW_CHANNEL.union(Permissions::CONNECT);

/// Запись в канал — то, что запрещается всем в каналах только для чтения.
pub const WRITE: Permissions = Permissions::SEND_MESSAGES
    .union(Permissions::SEND_MESSAGES_IN_THREADS)
    .union(Permissions::CREATE_PUBLIC_THREADS)
    .union(Permissions::CREATE_PRIVATE_THREADS)
    .union(Permissions::SEND_POLLS)
    .union(Permissions::SEND_VOICE_MESSAGES)
    .union(Permissions::SEND_TTS_MESSAGES);

/// Что получает неверифицированный в канале верификации: видеть, читать историю и нажимать
/// кнопки (команды приложений). Отправка сообщений — только если администрация разрешила её сама.
pub const UNVERIFIED_GATE: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::READ_MESSAGE_HISTORY)
    .union(Permissions::USE_APPLICATION_COMMANDS);

/// Биты, по которым выводится класс канала; при явном классе они снимаются с @everyone и
/// выставляются заново по классу.
const CLASS_BITS: Permissions = Permissions::VIEW_CHANNEL.union(Permissions::SEND_MESSAGES);

/// Назначение канала в модели.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelClass {
    /// Виден участникам, оверрайты нейтральны.
    Public,
    /// Виден только ролям из списка (и администраторам).
    Private { roles: BTreeSet<RoleId> },
    /// Виден участникам, пишут только авторы.
    ReadOnly { writers: BTreeSet<RoleId> },
    /// Канал верификации: виден неверифицированным, скрыт от участников.
    Verification,
    /// Бот не трогает канал (комнаты бота, тикеты, интеграции).
    Ignored,
}

impl ChannelClass {
    pub const KEYS: [&'static str; 5] =
        ["public", "private", "read-only", "verification", "ignored"];

    /// Значение в базе и в опциях команд.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private { .. } => "private",
            Self::ReadOnly { .. } => "read-only",
            Self::Verification => "verification",
            Self::Ignored => "ignored",
        }
    }

    /// Класс по ключу и списку ролей (для `private` — доступ, для `read-only` — авторы).
    pub fn from_key(key: &str, roles: BTreeSet<RoleId>) -> Option<Self> {
        Some(match key {
            "public" => Self::Public,
            "private" => Self::Private { roles },
            "read-only" => Self::ReadOnly { writers: roles },
            "verification" => Self::Verification,
            "ignored" => Self::Ignored,
            _ => return None,
        })
    }

    /// Роли, которые хранятся вместе с классом.
    pub fn roles(&self) -> Option<&BTreeSet<RoleId>> {
        match self {
            Self::Private { roles } | Self::ReadOnly { writers: roles } => Some(roles),
            Self::Public | Self::Verification | Self::Ignored => None,
        }
    }

    /// Нужен ли классу список ролей.
    pub fn takes_roles(key: &str) -> bool {
        matches!(key, "private" | "read-only")
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Public => "открытый",
            Self::Private { .. } => "закрытый",
            Self::ReadOnly { .. } => "только чтение",
            Self::Verification => "верификация",
            Self::Ignored => "не трогать",
        }
    }

    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Public => "🌐",
            Self::Private { .. } => "🔒",
            Self::ReadOnly { .. } => "📢",
            Self::Verification => "🛂",
            Self::Ignored => "⏸️",
        }
    }

    fn hides_from_everyone(&self) -> bool {
        matches!(self, Self::Private { .. } | Self::Verification)
    }
}

/// Назначение роли в политике.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyRole {
    Member,
    Unverified,
    ChatMute,
    VoiceMute,
}

impl PolicyRole {
    pub const ALL: [Self; 4] = [
        Self::Member,
        Self::Unverified,
        Self::ChatMute,
        Self::VoiceMute,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Unverified => "unverified",
            Self::ChatMute => "chat-mute",
            Self::VoiceMute => "voice-mute",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Member => "Участник",
            Self::Unverified => "Неверифицированный",
            Self::ChatMute => "Чат-мут",
            Self::VoiceMute => "Войс-мут",
        }
    }

    pub fn explain(self) -> &'static str {
        match self {
            Self::Member => "базовые права на уровне роли; выдаётся после верификации",
            Self::Unverified => "не даёт ничего; видит только каналы верификации",
            Self::ChatMute => "запрет писать и реагировать во всех каналах",
            Self::VoiceMute => "запрет говорить и включать видео во всех голосовых каналах",
        }
    }
}

/// Настройки политики сервера. Хранятся в базе, задаются администрацией.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Роль участника — единственная обязательная.
    pub member: RoleId,
    pub unverified: Option<RoleId>,
    pub chat_mute: Option<RoleId>,
    pub voice_mute: Option<RoleId>,
    /// Права @everyone на уровне сервера; по модели — ничего.
    pub everyone_permissions: Permissions,
    /// Права роли участника на уровне сервера.
    pub member_permissions: Permissions,
    /// Назначение каналов, заданное явно; остальные классифицируются по текущим оверрайтам.
    pub channels: BTreeMap<ChannelId, ChannelClass>,
}

impl Policy {
    pub fn new(member: RoleId) -> Self {
        Self {
            member,
            unverified: None,
            chat_mute: None,
            voice_mute: None,
            everyone_permissions: Permissions::empty(),
            member_permissions: MEMBER_BASELINE,
            channels: BTreeMap::new(),
        }
    }

    /// Роли политики с их назначением.
    pub fn roles(&self) -> Vec<(PolicyRole, RoleId)> {
        let mut roles = vec![(PolicyRole::Member, self.member)];
        roles.extend(self.unverified.map(|id| (PolicyRole::Unverified, id)));
        roles.extend(self.chat_mute.map(|id| (PolicyRole::ChatMute, id)));
        roles.extend(self.voice_mute.map(|id| (PolicyRole::VoiceMute, id)));
        roles
    }

    pub fn role(&self, purpose: PolicyRole) -> Option<RoleId> {
        match purpose {
            PolicyRole::Member => Some(self.member),
            PolicyRole::Unverified => self.unverified,
            PolicyRole::ChatMute => self.chat_mute,
            PolicyRole::VoiceMute => self.voice_mute,
        }
    }

    /// Назначает роль; `None` — убирает. Роль участника убрать нельзя.
    pub fn set_role(&mut self, purpose: PolicyRole, role: Option<RoleId>) -> bool {
        match (purpose, role) {
            (PolicyRole::Member, Some(id)) => self.member = id,
            (PolicyRole::Member, None) => return false,
            (PolicyRole::Unverified, id) => self.unverified = id,
            (PolicyRole::ChatMute, id) => self.chat_mute = id,
            (PolicyRole::VoiceMute, id) => self.voice_mute = id,
        }
        true
    }

    fn is_policy_role(&self, role: RoleId) -> bool {
        role == self.member
            || self.unverified == Some(role)
            || self.chat_mute == Some(role)
            || self.voice_mute == Some(role)
    }
}

/// Кому принадлежит оверрайт.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    Role(RoleId),
    Member(UserId),
    /// Вид оверрайта, неизвестный serenity 0.12: план его не трогает.
    Other,
}

impl From<PermissionOverwriteType> for Target {
    fn from(kind: PermissionOverwriteType) -> Self {
        match kind {
            PermissionOverwriteType::Role(id) => Self::Role(id),
            PermissionOverwriteType::Member(id) => Self::Member(id),
            _ => Self::Other,
        }
    }
}

/// Оверрайт канала.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overwrite {
    pub target: Target,
    pub allow: Permissions,
    pub deny: Permissions,
}

/// Канал в объёме, нужном для политики.
#[derive(Clone, Debug)]
pub struct ChannelSnap {
    pub id: ChannelId,
    pub kind: ChannelType,
    pub parent: Option<ChannelId>,
    pub position: u16,
    pub overwrites: Vec<Overwrite>,
    /// Права бота в этом канале (с учётом оверрайтов).
    pub bot: Permissions,
}

impl ChannelSnap {
    pub fn of(guild: &Guild, channel: &GuildChannel, bot: &Member) -> Self {
        Self {
            id: channel.id,
            kind: channel.kind,
            parent: channel.parent_id,
            position: channel.position,
            overwrites: channel
                .permission_overwrites
                .iter()
                .map(|overwrite| Overwrite {
                    target: overwrite.kind.into(),
                    allow: overwrite.allow,
                    deny: overwrite.deny,
                })
                .collect(),
            bot: guild.user_permissions_in(channel, bot),
        }
    }

    /// Разрешения и запреты роли в канале; без оверрайта — пустые.
    pub fn role_overwrite(&self, role: RoleId) -> Option<(Permissions, Permissions)> {
        self.overwrites
            .iter()
            .find(|overwrite| overwrite.target == Target::Role(role))
            .map(|overwrite| (overwrite.allow, overwrite.deny))
    }

    fn role_overwrites(&self) -> impl Iterator<Item = (RoleId, Permissions, Permissions)> + '_ {
        self.overwrites
            .iter()
            .filter_map(|overwrite| match overwrite.target {
                Target::Role(role) => Some((role, overwrite.allow, overwrite.deny)),
                Target::Member(_) | Target::Other => None,
            })
    }

    fn member_overwrites(&self) -> usize {
        self.overwrites
            .iter()
            .filter(|overwrite| !matches!(overwrite.target, Target::Role(_)))
            .count()
    }

    pub fn is_thread(&self) -> bool {
        matches!(
            self.kind,
            ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
        )
    }

    fn voice_like(&self) -> bool {
        matches!(self.kind, ChannelType::Voice | ChannelType::Stage)
    }
}

/// Снимок сервера для планирования.
pub struct Snapshot {
    pub hierarchy: Hierarchy,
    /// Категории, затем каналы — по категории и позиции.
    pub channels: Vec<ChannelSnap>,
    pub bot_id: UserId,
    pub bot_roles: Vec<RoleId>,
}

impl Snapshot {
    /// Из кэша сервера и участника-бота (его роли читаются из Discord).
    pub fn of(guild: &Guild, bot: &Member) -> Self {
        let channels = guild
            .channels
            .values()
            .map(|channel| ChannelSnap::of(guild, channel, bot))
            .collect();
        Self::new(
            Hierarchy::of(guild),
            channels,
            bot.user.id,
            bot.roles.clone(),
        )
    }

    pub fn new(
        hierarchy: Hierarchy,
        mut channels: Vec<ChannelSnap>,
        bot_id: UserId,
        bot_roles: Vec<RoleId>,
    ) -> Self {
        // Категории — первыми, затем каналы по категории и позиции: порядок плана.
        let category_position: BTreeMap<ChannelId, u16> = channels
            .iter()
            .filter(|channel| channel.kind == ChannelType::Category)
            .map(|channel| (channel.id, channel.position))
            .collect();
        channels.sort_by_key(|channel| {
            let parent = channel
                .parent
                .and_then(|id| category_position.get(&id).copied());
            (
                channel.kind != ChannelType::Category,
                parent.unwrap_or(0),
                channel.parent.map_or(0, |id| id.get()),
                channel.position,
                channel.id.get(),
            )
        });
        Self {
            hierarchy,
            channels,
            bot_id,
            bot_roles,
        }
    }

    pub fn bot(&self) -> Person<'_> {
        Person {
            id: self.bot_id,
            roles: &self.bot_roles,
            bot: true,
        }
    }

    #[cfg(test)]
    pub fn channel(&self, id: ChannelId) -> Option<&ChannelSnap> {
        self.channels.iter().find(|channel| channel.id == id)
    }
}

/// Операция плана.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    EditRole {
        role: RoleId,
        before: Permissions,
        after: Permissions,
    },
    SetOverwrite {
        channel: ChannelId,
        role: RoleId,
        before: Option<(Permissions, Permissions)>,
        allow: Permissions,
        deny: Permissions,
    },
    DeleteOverwrite {
        channel: ChannelId,
        role: RoleId,
        before: (Permissions, Permissions),
    },
}

impl Op {
    pub fn channel(&self) -> Option<ChannelId> {
        match self {
            Self::EditRole { .. } => None,
            Self::SetOverwrite { channel, .. } | Self::DeleteOverwrite { channel, .. } => {
                Some(*channel)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    /// План нельзя применять: не выполнены предпосылки.
    Blocker,
}

impl Severity {
    pub fn emoji(self) -> &'static str {
        match self {
            Self::Info => "ℹ️",
            Self::Warning => "⚠️",
            Self::Blocker => "⛔",
        }
    }
}

/// Замечание к состоянию сервера или к операции плана.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub text: String,
}

/// Итог планирования.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub ops: Vec<Op>,
    pub findings: Vec<Finding>,
    /// Действующий класс каждого канала: заданный явно или выведенный из оверрайтов.
    pub classes: BTreeMap<ChannelId, ChannelClass>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn blocked(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == Severity::Blocker)
    }

    /// Отпечаток операций: по нему проверяется, что применяется именно показанный план.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = Fnv1a(0xcbf2_9ce4_8422_2325);
        self.ops.hash(&mut hasher);
        hasher.0
    }

    fn note(&mut self, severity: Severity, text: impl Into<String>) {
        self.findings.push(Finding {
            severity,
            text: text.into(),
        });
    }
}

/// FNV-1a: детерминированный хеш, не зависящий от версии стандартной библиотеки.
struct Fnv1a(u64);

impl Hasher for Fnv1a {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Класс канала по его текущим оверрайтам. Запреты роли участника учитываются как запреты
/// @everyone: в модели они переносятся туда.
pub fn infer_class(channel: &ChannelSnap, policy: &Policy, everyone: RoleId) -> ChannelClass {
    let (_, deny_everyone) = channel.role_overwrite(everyone).unwrap_or_default();
    let (_, deny_member) = channel.role_overwrite(policy.member).unwrap_or_default();
    let deny = deny_everyone | deny_member;

    if deny.view_channel() {
        let roles: BTreeSet<RoleId> = channel
            .role_overwrites()
            .filter(|&(role, allow, _)| role != everyone && allow.view_channel())
            .map(|(role, _, _)| role)
            .collect();
        if let Some(unverified) = policy.unverified
            && roles.contains(&unverified)
            && !roles.contains(&policy.member)
        {
            return ChannelClass::Verification;
        }
        return ChannelClass::Private { roles };
    }

    if !channel.voice_like() && deny.send_messages() {
        let writers = channel
            .role_overwrites()
            .filter(|&(role, allow, _)| role != everyone && allow.intersects(WRITE))
            .map(|(role, _, _)| role)
            .collect();
        return ChannelClass::ReadOnly { writers };
    }
    ChannelClass::Public
}

/// План приведения сервера к политике.
pub fn plan(snapshot: &Snapshot, policy: &Policy) -> Plan {
    let mut plan = Plan::default();
    let hierarchy = &snapshot.hierarchy;
    let everyone = hierarchy.everyone();
    let bot = snapshot.bot();
    let bot_permissions = hierarchy.permissions(bot);
    let bot_height = hierarchy.height(bot);
    let admin = bot_permissions.administrator();

    if !bot_permissions.manage_roles() {
        plan.note(
            Severity::Blocker,
            "У бота нет права «Управлять ролями»: без него нельзя менять ни роли, ни оверрайты.",
        );
    }

    // Роли политики: существуют, обычные, различны.
    let mut seen = BTreeSet::new();
    for (purpose, id) in policy.roles() {
        if !seen.insert(id) {
            plan.note(
                Severity::Blocker,
                format!("{} указана в политике дважды.", id.mention()),
            );
            continue;
        }
        match hierarchy.role(id) {
            None => plan.note(
                Severity::Blocker,
                format!(
                    "Роль «{}» ({}) не найдена на сервере.",
                    purpose.label(),
                    id.mention()
                ),
            ),
            Some(role) if hierarchy.is_everyone(id) || role.origin.is_managed() => plan.note(
                Severity::Blocker,
                format!(
                    "{} не может быть ролью «{}»: это {}.",
                    id.mention(),
                    purpose.label(),
                    role.origin.label()
                ),
            ),
            Some(_) => {}
        }
    }
    if plan.blocked() {
        return plan;
    }

    // Права ролей на уровне сервера.
    let mut targets = vec![(everyone, policy.everyone_permissions, "@everyone")];
    for (purpose, id) in policy.roles() {
        let target = match purpose {
            PolicyRole::Member => policy.member_permissions,
            PolicyRole::Unverified | PolicyRole::ChatMute | PolicyRole::VoiceMute => {
                Permissions::empty()
            }
        };
        targets.push((id, target, purpose.label()));
    }
    for (id, target, label) in targets {
        let Some(role) = hierarchy.role(id) else {
            continue;
        };
        let before = role.permissions;
        if (before ^ target).intersection(OBSERVABLE).is_empty() {
            continue;
        }
        if !hierarchy.is_everyone(id) && !bot_height.above(role) {
            plan.note(
                Severity::Blocker,
                format!(
                    "{} не ниже роли бота — Discord не даст изменить её права. Поднимите роль бота.",
                    id.mention()
                ),
            );
            continue;
        }
        let removed = (before - target) & OBSERVABLE;
        let added = target - before;
        let mut text = format!("Права роли {} ({label}):", id.mention());
        if !removed.is_empty() {
            text += &format!(" снять {}", describe(removed));
        }
        if !added.is_empty() {
            let separator = if removed.is_empty() { "" } else { ";" };
            text += &format!("{separator} выдать {}", describe(added));
        }
        let severity = if removed.intersects(permissions::STAFF) {
            Severity::Warning
        } else {
            Severity::Info
        };
        plan.note(severity, text + ".");
        plan.ops.push(Op::EditRole {
            role: id,
            before,
            after: target,
        });
    }
    if plan.blocked() {
        plan.ops.clear();
        return plan;
    }

    // Обзор остальных ролей: администрация по уровням и интеграции с опасными правами.
    let staff: BTreeSet<RoleId> = hierarchy.staff().into_iter().map(|role| role.id).collect();
    let mut by_tier: BTreeMap<Tier, Vec<RoleId>> = BTreeMap::new();
    for role in hierarchy.sorted() {
        if policy.is_policy_role(role.id) {
            continue;
        }
        let class = role.class();
        if class.is_staff() {
            by_tier.entry(class.tier).or_default().push(role.id);
        } else if role.origin.is_managed() && role.permissions.intersects(permissions::STAFF) {
            plan.note(
                Severity::Info,
                format!(
                    "{} ({}) имеет {} — права интеграций бот не меняет.",
                    role.id.mention(),
                    role.origin.label(),
                    describe(role.permissions & permissions::STAFF)
                ),
            );
        }
    }
    for (tier, roles) in &by_tier {
        plan.note(
            Severity::Info,
            format!(
                "{} {}: {}.",
                tier.emoji(),
                tier.label(),
                hierarchy::mentions(roles)
            ),
        );
    }

    // Каналы.
    let mut personal = (0usize, 0usize);
    let mut skipped = Vec::new();
    for channel in &snapshot.channels {
        if channel.is_thread() {
            continue;
        }
        let class = policy
            .channels
            .get(&channel.id)
            .cloned()
            .unwrap_or_else(|| infer_class(channel, policy, everyone));
        plan.classes.insert(channel.id, class.clone());
        if class == ChannelClass::Ignored {
            continue;
        }
        let members = channel.member_overwrites();
        if members > 0 {
            personal.0 += members;
            personal.1 += 1;
        }
        if !admin
            && !channel
                .bot
                .contains(Permissions::VIEW_CHANNEL | Permissions::MANAGE_ROLES)
        {
            skipped.push(channel.id);
            continue;
        }
        let editable = if admin {
            permissions::KNOWN
        } else {
            channel.bot
        };
        let planner = ChannelPlanner {
            channel,
            class: &class,
            policy,
            everyone,
            staff: &staff,
            editable,
        };
        planner.run(&mut plan);
    }
    if !skipped.is_empty() {
        plan.note(
            Severity::Warning,
            format!(
                "Бот не может менять права в каналах (нет «Просматривать каналы» или «Управлять \
                 ролями» в них), они пропущены: {}.",
                skipped
                    .iter()
                    .map(|id| id.mention().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    if personal.0 > 0 {
        plan.note(
            Severity::Info,
            format!(
                "Персональные оверрайты участников не затрагиваются: {} в {} каналах.",
                personal.0, personal.1
            ),
        );
    }
    plan
}

/// Планирование одного канала: целевые оверрайты ролей и операции для их достижения.
struct ChannelPlanner<'a> {
    channel: &'a ChannelSnap,
    class: &'a ChannelClass,
    policy: &'a Policy,
    everyone: RoleId,
    staff: &'a BTreeSet<RoleId>,
    /// Права, которые бот может выставлять в этом канале.
    editable: Permissions,
}

impl ChannelPlanner<'_> {
    fn run(&self, plan: &mut Plan) {
        let desired = self.desired(plan);
        self.emit(plan, &desired);
    }

    /// Целевые оверрайты: роль → (allow, deny). Роль вне карты — оверрайт удаляется.
    fn desired(&self, plan: &mut Plan) -> BTreeMap<RoleId, (Permissions, Permissions)> {
        let channel = self.channel;
        let policy = self.policy;
        let applicable = permissions::applicable(channel.kind);
        let mention = channel.id.mention();
        let mut desired = BTreeMap::new();

        // @everyone: запреты сохраняются (плюс перенесённые с роли участника), классовые биты
        // выставляются по классу, разрешений нет.
        let (allow_everyone, deny_everyone) =
            channel.role_overwrite(self.everyone).unwrap_or_default();
        let (_, deny_member) = channel.role_overwrite(policy.member).unwrap_or_default();
        let migrated = (deny_member & applicable) - deny_everyone;
        if !migrated.is_empty() {
            plan.note(
                Severity::Info,
                format!(
                    "{mention}: запреты роли участника ({}) перенесены на @everyone — так они \
                     действуют и на всех остальных.",
                    describe(migrated)
                ),
            );
        }
        if !(allow_everyone & applicable).is_empty() {
            plan.note(
                Severity::Warning,
                format!(
                    "{mention}: у @everyone было разрешение {} — в модели оно открывало бы канал \
                     неверифицированным; снято.",
                    describe(allow_everyone & applicable)
                ),
            );
        }
        let strip = match self.class {
            ChannelClass::Public => CLASS_BITS,
            _ => Permissions::VIEW_CHANNEL,
        };
        let preserved = ((deny_everyone | deny_member) & applicable) - strip;
        let everyone_deny = match self.class {
            ChannelClass::Public | ChannelClass::Ignored => preserved,
            ChannelClass::Private { .. } | ChannelClass::Verification => {
                preserved | Permissions::VIEW_CHANNEL
            }
            ChannelClass::ReadOnly { .. } => preserved | (WRITE & applicable),
        };
        desired.insert(self.everyone, (Permissions::empty(), everyone_deny));

        // Список доступа и авторы по классу.
        let (whitelist, writers): (BTreeSet<RoleId>, BTreeSet<RoleId>) = match self.class {
            ChannelClass::Private { roles } => (roles.clone(), BTreeSet::new()),
            ChannelClass::Verification => {
                // Кто видит канал сейчас (кроме участников) плюс неверифицированный.
                let mut roles: BTreeSet<RoleId> = channel
                    .role_overwrites()
                    .filter(|&(role, allow, _)| role != self.everyone && allow.view_channel())
                    .map(|(role, _, _)| role)
                    .collect();
                roles.remove(&policy.member);
                roles.extend(policy.unverified);
                if policy.unverified.is_none() {
                    plan.note(
                        Severity::Warning,
                        format!(
                            "{mention}: класс «верификация» без роли неверифицированного — канал \
                             считается закрытым."
                        ),
                    );
                }
                (roles, BTreeSet::new())
            }
            ChannelClass::ReadOnly { writers } => (BTreeSet::new(), writers.clone()),
            ChannelClass::Public | ChannelClass::Ignored => (BTreeSet::new(), BTreeSet::new()),
        };

        for (role, allow, deny) in channel.role_overwrites() {
            if role == self.everyone {
                continue;
            }
            let allow = allow & applicable;
            let deny = deny & applicable;
            let gated = whitelist.contains(&role);
            let writer = writers.contains(&role);

            if role == policy.member {
                // Участник: разрешения излишни (права на уровне роли), запреты уже перенесены.
                if !allow.is_empty() && !gated && !writer {
                    let extra = allow - policy.member_permissions;
                    if extra.is_empty() {
                        plan.note(
                            Severity::Info,
                            format!(
                                "{mention}: разрешения роли участника ({}) излишни — они есть на \
                                 уровне роли; сняты.",
                                describe(allow)
                            ),
                        );
                    } else {
                        plan.note(
                            Severity::Warning,
                            format!(
                                "{mention}: роль участника имела {}, чего нет в базовых правах; \
                                 снято — выдайте на уровне роли или назначьте канал «только \
                                 чтение» с авторами.",
                                describe(extra)
                            ),
                        );
                    }
                }
                continue;
            }
            if policy.chat_mute == Some(role) || policy.voice_mute == Some(role) {
                continue; // муты задаются ниже
            }
            if *self.class == ChannelClass::Verification && policy.unverified == Some(role) {
                continue; // задаётся ниже
            }

            // Закрытый канал: оверрайт роли вне списка доступа не действует (просмотр запрещён
            // всем) — удаляется.
            if self.class.hides_from_everyone() && !gated {
                if !allow.is_empty() || !deny.is_empty() {
                    plan.note(
                        Severity::Info,
                        format!(
                            "{mention}: оверрайт {} не действует без доступа к каналу; снят.",
                            role.mention()
                        ),
                    );
                }
                continue;
            }

            // Разрешение в канале законно, если открывает ворота или снимает запрет @everyone;
            // иное либо излишне, либо перебивает муты. Администрации доверяем целиком.
            let mut kept_allow = allow;
            if !self.staff.contains(&role) {
                let overriding = (allow & MUTE_BITS) - everyone_deny;
                if !overriding.is_empty() {
                    plan.note(
                        Severity::Warning,
                        format!(
                            "{mention}: разрешение {} для {} перебивало бы муты; снято — эти \
                             права роль получает на уровне сервера.",
                            describe(overriding),
                            role.mention()
                        ),
                    );
                    kept_allow -= overriding;
                }
            }
            if gated {
                kept_allow |= GATE & applicable;
            }
            if writer {
                kept_allow |= WRITE & applicable;
            }
            if kept_allow.is_empty() && deny.is_empty() {
                continue;
            }
            desired.insert(role, (kept_allow, deny));
        }

        // Роли из списка доступа и авторы без существующего оверрайта.
        for &role in &whitelist {
            desired
                .entry(role)
                .or_insert((Permissions::empty(), Permissions::empty()))
                .0 |= GATE & applicable;
        }
        for &role in &writers {
            desired
                .entry(role)
                .or_insert((Permissions::empty(), Permissions::empty()))
                .0 |= WRITE & applicable;
            if !self.staff.contains(&role) {
                plan.note(
                    Severity::Info,
                    format!(
                        "{mention}: {} пишет в канале «только чтение», не входя в администрацию.",
                        role.mention()
                    ),
                );
            }
        }
        // Неверифицированный в канале верификации: ворота плюс то, что ему уже разрешили.
        if *self.class == ChannelClass::Verification
            && let Some(unverified) = policy.unverified
        {
            let (allow, deny) = channel.role_overwrite(unverified).unwrap_or_default();
            let entry = desired
                .entry(unverified)
                .or_insert((Permissions::empty(), Permissions::empty()));
            entry.0 |= (UNVERIFIED_GATE | allow) & applicable;
            entry.1 = deny & applicable;
        }
        // Муты: запрет во всех каналах; существующие запреты сохраняются, разрешения снимаются.
        // Войс-мут имеет смысл только в голосовых каналах и категориях.
        let voice_scope = channel.voice_like() || channel.kind == ChannelType::Category;
        let mutes = [
            (policy.chat_mute, CHAT_MUTE, true),
            (policy.voice_mute, VOICE_MUTE, voice_scope),
        ];
        for (role, bits, applies) in mutes {
            let Some(role) = role else {
                continue;
            };
            let (allow, deny) = channel.role_overwrite(role).unwrap_or_default();
            if !(allow & applicable).is_empty() {
                plan.note(
                    Severity::Warning,
                    format!(
                        "{mention}: роль мута {} имела разрешение {} — снято.",
                        role.mention(),
                        describe(allow & applicable)
                    ),
                );
            }
            let target = if applies { bits | deny } else { deny } & applicable;
            if target.is_empty() {
                desired.remove(&role);
            } else {
                desired.insert(role, (Permissions::empty(), target));
            }
        }
        desired
    }

    /// Операции: различия только в наблюдаемых битах и только в правах, доступных боту.
    fn emit(&self, plan: &mut Plan, desired: &BTreeMap<RoleId, (Permissions, Permissions)>) {
        let channel = self.channel;
        for (&role, &(allow, deny)) in desired {
            let before = channel.role_overwrite(role);
            if allow.is_empty() && deny.is_empty() {
                if let Some(before) = before {
                    plan.ops.push(Op::DeleteOverwrite {
                        channel: channel.id,
                        role,
                        before,
                    });
                }
                continue;
            }
            let beyond = (allow | deny) - self.editable;
            let (allow, deny) = (allow - beyond, deny - beyond);
            if !beyond.is_empty() {
                plan.note(
                    Severity::Warning,
                    format!(
                        "{}: боту не хватает {} в канале, чтобы выставить их для {}; эти биты \
                         пропущены.",
                        channel.id.mention(),
                        describe(beyond & OBSERVABLE),
                        role.mention()
                    ),
                );
                if allow.is_empty() && deny.is_empty() {
                    continue;
                }
            }
            let same = before.is_some_and(|(a, d)| {
                ((a ^ allow) | (d ^ deny))
                    .intersection(OBSERVABLE)
                    .is_empty()
            });
            if !same {
                plan.ops.push(Op::SetOverwrite {
                    channel: channel.id,
                    role,
                    before,
                    allow,
                    deny,
                });
            }
        }
        for (role, allow, deny) in channel.role_overwrites() {
            if !desired.contains_key(&role) {
                plan.ops.push(Op::DeleteOverwrite {
                    channel: channel.id,
                    role,
                    before: (allow, deny),
                });
            }
        }
    }
}

/// Состояние после применения операций — для проверки идемпотентности и предпросмотра.
pub fn simulate(snapshot: &Snapshot, ops: &[Op]) -> Snapshot {
    let mut hierarchy = Hierarchy::new(
        snapshot.hierarchy.guild_id(),
        snapshot.hierarchy.owner(),
        snapshot.hierarchy.roles().cloned(),
    );
    let mut channels = snapshot.channels.clone();
    for op in ops {
        match op {
            Op::EditRole { role, after, .. } => {
                hierarchy.set_permissions(*role, *after);
            }
            Op::SetOverwrite {
                channel,
                role,
                allow,
                deny,
                ..
            } => {
                if let Some(channel) = channels.iter_mut().find(|c| c.id == *channel) {
                    let target = Target::Role(*role);
                    channel.overwrites.retain(|o| o.target != target);
                    channel.overwrites.push(Overwrite {
                        target,
                        allow: *allow,
                        deny: *deny,
                    });
                }
            }
            Op::DeleteOverwrite { channel, role, .. } => {
                if let Some(channel) = channels.iter_mut().find(|c| c.id == *channel) {
                    channel
                        .overwrites
                        .retain(|o| o.target != Target::Role(*role));
                }
            }
        }
    }
    Snapshot::new(
        hierarchy,
        channels,
        snapshot.bot_id,
        snapshot.bot_roles.clone(),
    )
}

/// Права участника в канале по алгоритму Discord: права @everyone и ролей на уровне сервера →
/// оверрайт @everyone → оверрайты ролей (все запреты, затем все разрешения) → персональный
/// оверрайт. Администратор получает всё.
pub fn effective(
    snapshot: &Snapshot,
    user: UserId,
    roles: &[RoleId],
    channel: &ChannelSnap,
) -> Permissions {
    let hierarchy = &snapshot.hierarchy;
    let base = hierarchy.permissions(Person {
        id: user,
        roles,
        bot: false,
    });
    if base.administrator() {
        return Permissions::all();
    }
    let mut granted = base;
    if let Some((allow, deny)) = channel.role_overwrite(hierarchy.everyone()) {
        granted = (granted - deny) | allow;
    }
    let (mut allow, mut deny) = (Permissions::empty(), Permissions::empty());
    for &role in roles {
        if let Some((a, d)) = channel.role_overwrite(role) {
            allow |= a;
            deny |= d;
        }
    }
    granted = (granted - deny) | allow;
    if let Some(overwrite) = channel
        .overwrites
        .iter()
        .find(|o| o.target == Target::Member(user))
    {
        granted = (granted - overwrite.deny) | overwrite.allow;
    }
    // Неявные запреты Discord: без просмотра канала не действует ничего, без отправки
    // сообщений — упоминания, TTS, файлы и ссылки.
    let missing = CLASS_BITS - granted;
    granted - permissions::implied_denies(missing)
}

/// Проверка модели на состоянии сервера (обычно — смоделированном после плана): что роли
/// политики должны и не должны видеть и делать. Пустой `failing` — проверка пройдена.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub title: &'static str,
    pub failing: Vec<ChannelId>,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.failing.is_empty()
    }
}

/// Проверки модели по каналам с известным классом, кроме игнорируемых, категорий и веток.
/// Участник-проба не имеет персональных оверрайтов.
pub fn verify(
    snapshot: &Snapshot,
    policy: &Policy,
    classes: &BTreeMap<ChannelId, ChannelClass>,
) -> Vec<Check> {
    let probe = UserId::new(u64::MAX);
    let channels: Vec<(&ChannelSnap, &ChannelClass)> = snapshot
        .channels
        .iter()
        .filter(|channel| !channel.is_thread() && channel.kind != ChannelType::Category)
        .filter_map(|channel| classes.get(&channel.id).map(|class| (channel, class)))
        .filter(|(_, class)| **class != ChannelClass::Ignored)
        .collect();
    let eff = |roles: &[RoleId], channel: &ChannelSnap| effective(snapshot, probe, roles, channel);
    let failing = |keep: &dyn Fn(&ChannelSnap, &ChannelClass) -> bool| -> Vec<ChannelId> {
        channels
            .iter()
            .filter(|(channel, class)| keep(channel, class))
            .map(|(channel, _)| channel.id)
            .collect()
    };
    let member = policy.member;

    let mut checks = vec![
        Check {
            title: "Без ролей не виден ни один канал",
            failing: failing(&|channel, _| eff(&[], channel).view_channel()),
        },
        Check {
            title: "Участник видит все открытые каналы и каналы только для чтения",
            failing: failing(&|channel, class| {
                matches!(class, ChannelClass::Public | ChannelClass::ReadOnly { .. })
                    && !eff(&[member], channel).view_channel()
            }),
        },
    ];
    if let Some(unverified) = policy.unverified {
        checks.push(Check {
            title: "Неверифицированный видит только каналы верификации",
            failing: failing(&|channel, class| {
                eff(&[unverified], channel).view_channel() != (*class == ChannelClass::Verification)
            }),
        });
    }
    if let Some(mute) = policy.chat_mute {
        checks.push(Check {
            title: "Чат-мут запрещает писать везде, где участник может",
            failing: failing(&|channel, _| {
                let applicable = permissions::applicable(channel.kind);
                eff(&[member], channel).send_messages()
                    && eff(&[member, mute], channel).intersects(CHAT_MUTE & applicable)
            }),
        });
    }
    if let Some(mute) = policy.voice_mute {
        checks.push(Check {
            title: "Войс-мут: подключаться можно, говорить нельзя",
            failing: failing(&|channel, _| {
                let applicable = permissions::applicable(channel.kind);
                let muted = eff(&[member, mute], channel);
                channel.voice_like()
                    && eff(&[member], channel).contains(Permissions::CONNECT | Permissions::SPEAK)
                    && !(muted.connect() && !muted.intersects(VOICE_MUTE & applicable))
            }),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use serenity::all::GuildId;

    use super::*;
    use crate::domain::hierarchy::RoleInfo;
    use crate::domain::roles::Origin;

    const GUILD: GuildId = GuildId::new(1);
    const EVERYONE: RoleId = RoleId::new(1);
    const OWNER: UserId = UserId::new(100);
    const BOT: UserId = UserId::new(200);

    const MEMBER: RoleId = RoleId::new(10);
    const UNVERIFIED: RoleId = RoleId::new(11);
    const CHAT_MUTE_ROLE: RoleId = RoleId::new(12);
    const VOICE_MUTE_ROLE: RoleId = RoleId::new(13);
    const MOD: RoleId = RoleId::new(20);
    const ADMIN: RoleId = RoleId::new(21);
    const BOT_ROLE: RoleId = RoleId::new(30);
    const CLUB: RoleId = RoleId::new(40);

    const CATEGORY: ChannelId = ChannelId::new(100);
    const GENERAL: ChannelId = ChannelId::new(101);
    const RULES: ChannelId = ChannelId::new(102);
    const STAFF_CHAT: ChannelId = ChannelId::new(103);
    const NEWS: ChannelId = ChannelId::new(104);
    const VOICE: ChannelId = ChannelId::new(105);
    const CLUB_CHAT: ChannelId = ChannelId::new(106);
    const HUB: ChannelId = ChannelId::new(107);

    const P_VIEW: Permissions = Permissions::VIEW_CHANNEL;
    const P_SEND: Permissions = Permissions::SEND_MESSAGES;

    fn role(id: RoleId, position: u16, permissions: Permissions, origin: Origin) -> RoleInfo {
        RoleInfo {
            id,
            position,
            permissions,
            origin,
        }
    }

    fn overwrite(role: RoleId, allow: Permissions, deny: Permissions) -> Overwrite {
        Overwrite {
            target: Target::Role(role),
            allow,
            deny,
        }
    }

    fn channel(
        id: ChannelId,
        kind: ChannelType,
        parent: Option<ChannelId>,
        overwrites: Vec<Overwrite>,
    ) -> ChannelSnap {
        ChannelSnap {
            id,
            kind,
            parent,
            position: id.get() as u16,
            overwrites,
            bot: Permissions::all() - Permissions::ADMINISTRATOR,
        }
    }

    /// Сервер «до»: @everyone с правами, роль участника без прав, разрешения вместо запретов.
    fn snapshot() -> Snapshot {
        let hierarchy = Hierarchy::new(
            GUILD,
            OWNER,
            [
                role(EVERYONE, 0, P_VIEW | P_SEND, Origin::Everyone),
                role(MEMBER, 1, Permissions::empty(), Origin::Regular),
                role(UNVERIFIED, 2, P_SEND, Origin::Regular),
                role(CHAT_MUTE_ROLE, 3, Permissions::empty(), Origin::Regular),
                role(VOICE_MUTE_ROLE, 4, Permissions::empty(), Origin::Regular),
                role(CLUB, 5, Permissions::empty(), Origin::Regular),
                role(
                    MOD,
                    6,
                    Permissions::BAN_MEMBERS | Permissions::MANAGE_MESSAGES | P_VIEW | P_SEND,
                    Origin::Regular,
                ),
                role(ADMIN, 9, Permissions::ADMINISTRATOR, Origin::Regular),
                role(
                    BOT_ROLE,
                    8,
                    Permissions::MANAGE_ROLES | P_VIEW,
                    Origin::Integration,
                ),
            ],
        );
        let channels = vec![
            channel(CATEGORY, ChannelType::Category, None, vec![]),
            channel(
                GENERAL,
                ChannelType::Text,
                Some(CATEGORY),
                vec![overwrite(MEMBER, P_VIEW | P_SEND, Permissions::empty())],
            ),
            channel(
                RULES,
                ChannelType::Text,
                Some(CATEGORY),
                vec![
                    overwrite(EVERYONE, Permissions::empty(), P_VIEW),
                    overwrite(
                        UNVERIFIED,
                        P_VIEW | Permissions::READ_MESSAGE_HISTORY,
                        Permissions::empty(),
                    ),
                    overwrite(MOD, P_VIEW, Permissions::empty()),
                ],
            ),
            channel(
                STAFF_CHAT,
                ChannelType::Text,
                Some(CATEGORY),
                vec![
                    overwrite(EVERYONE, Permissions::empty(), P_VIEW),
                    overwrite(
                        MOD,
                        P_VIEW | P_SEND | Permissions::MANAGE_MESSAGES,
                        Permissions::empty(),
                    ),
                    overwrite(ADMIN, P_VIEW, Permissions::empty()),
                    Overwrite {
                        target: Target::Member(UserId::new(555)),
                        allow: P_VIEW,
                        deny: Permissions::empty(),
                    },
                ],
            ),
            channel(
                NEWS,
                ChannelType::News,
                Some(CATEGORY),
                vec![
                    overwrite(EVERYONE, Permissions::empty(), P_SEND),
                    overwrite(MOD, P_SEND, Permissions::empty()),
                ],
            ),
            channel(VOICE, ChannelType::Voice, Some(CATEGORY), vec![]),
            channel(
                CLUB_CHAT,
                ChannelType::Text,
                None,
                vec![
                    overwrite(EVERYONE, Permissions::empty(), P_VIEW),
                    overwrite(CLUB, P_VIEW | P_SEND, Permissions::empty()),
                ],
            ),
            channel(
                HUB,
                ChannelType::Voice,
                None,
                vec![overwrite(
                    EVERYONE,
                    Permissions::empty(),
                    Permissions::SPEAK,
                )],
            ),
        ];
        Snapshot::new(hierarchy, channels, BOT, vec![BOT_ROLE])
    }

    fn policy() -> Policy {
        let mut policy = Policy::new(MEMBER);
        policy.unverified = Some(UNVERIFIED);
        policy.chat_mute = Some(CHAT_MUTE_ROLE);
        policy.voice_mute = Some(VOICE_MUTE_ROLE);
        policy.channels.insert(HUB, ChannelClass::Ignored);
        policy
    }

    fn ops_for(plan: &Plan, channel: ChannelId) -> Vec<&Op> {
        plan.ops
            .iter()
            .filter(|op| op.channel() == Some(channel))
            .collect()
    }

    fn set_op(plan: &Plan, channel: ChannelId, role: RoleId) -> Option<(Permissions, Permissions)> {
        plan.ops.iter().find_map(|op| match op {
            Op::SetOverwrite {
                channel: c,
                role: r,
                allow,
                deny,
                ..
            } if *c == channel && *r == role => Some((*allow, *deny)),
            _ => None,
        })
    }

    fn deletes(plan: &Plan, channel: ChannelId, role: RoleId) -> bool {
        plan.ops.iter().any(|op| {
            matches!(op, Op::DeleteOverwrite { channel: c, role: r, .. } if *c == channel && *r == role)
        })
    }

    /// Инвариант мутов на снимке: разрешения неадминистративных ролей в канале не выдают
    /// `MUTE_BITS` сверх запрета @everyone.
    fn mute_invariant_holds(snapshot: &Snapshot, staff: &BTreeSet<RoleId>) -> bool {
        let everyone = snapshot.hierarchy.everyone();
        snapshot.channels.iter().all(|channel| {
            let (_, everyone_deny) = channel.role_overwrite(everyone).unwrap_or_default();
            channel
                .role_overwrites()
                .filter(|&(role, _, _)| role != everyone && !staff.contains(&role))
                .all(|(_, allow, _)| ((allow & MUTE_BITS) - everyone_deny).is_empty())
        })
    }

    #[test]
    fn role_level_permissions_follow_the_model() {
        let plan = plan(&snapshot(), &policy());
        assert!(!plan.blocked(), "{:?}", plan.findings);
        let edits: BTreeMap<RoleId, Permissions> = plan
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::EditRole { role, after, .. } => Some((*role, *after)),
                _ => None,
            })
            .collect();
        assert_eq!(edits[&EVERYONE], Permissions::empty());
        assert_eq!(edits[&MEMBER], MEMBER_BASELINE);
        assert_eq!(edits[&UNVERIFIED], Permissions::empty());
        assert!(!edits.contains_key(&CHAT_MUTE_ROLE), "уже пустая");
        assert!(!edits.contains_key(&MOD), "чужие роли не трогаем");
        assert!(!edits.contains_key(&ADMIN));
        // Роли идут первыми.
        assert!(matches!(plan.ops[0], Op::EditRole { .. }));
    }

    #[test]
    fn classes_are_inferred_from_overwrites() {
        let plan = plan(&snapshot(), &policy());
        assert_eq!(plan.classes[&CATEGORY], ChannelClass::Public);
        assert_eq!(plan.classes[&GENERAL], ChannelClass::Public);
        assert_eq!(plan.classes[&RULES], ChannelClass::Verification);
        assert_eq!(
            plan.classes[&STAFF_CHAT],
            ChannelClass::Private {
                roles: BTreeSet::from([MOD, ADMIN])
            }
        );
        assert_eq!(
            plan.classes[&NEWS],
            ChannelClass::ReadOnly {
                writers: BTreeSet::from([MOD])
            }
        );
        assert_eq!(plan.classes[&VOICE], ChannelClass::Public);
        assert_eq!(
            plan.classes[&CLUB_CHAT],
            ChannelClass::Private {
                roles: BTreeSet::from([CLUB])
            }
        );
        assert_eq!(plan.classes[&HUB], ChannelClass::Ignored);
        assert!(
            ops_for(&plan, HUB).is_empty(),
            "игнорируемый канал не трогаем"
        );
    }

    #[test]
    fn public_channel_becomes_neutral_with_mute_denies() {
        let plan = plan(&snapshot(), &policy());
        assert!(
            deletes(&plan, GENERAL, MEMBER),
            "излишнее разрешение участника снято"
        );
        let text = permissions::applicable(ChannelType::Text);
        assert_eq!(
            set_op(&plan, GENERAL, CHAT_MUTE_ROLE),
            Some((Permissions::empty(), CHAT_MUTE & text))
        );
        assert_eq!(
            set_op(&plan, GENERAL, VOICE_MUTE_ROLE),
            None,
            "войс-мут не для текста"
        );
        assert_eq!(
            set_op(&plan, GENERAL, EVERYONE),
            None,
            "у @everyone нечего запрещать"
        );

        let voice = permissions::applicable(ChannelType::Voice);
        assert_eq!(
            set_op(&plan, VOICE, CHAT_MUTE_ROLE),
            Some((Permissions::empty(), CHAT_MUTE & voice))
        );
        assert_eq!(
            set_op(&plan, VOICE, VOICE_MUTE_ROLE),
            Some((Permissions::empty(), VOICE_MUTE & voice))
        );
        // Категория получает оба запрета — каналы в ней синхронизируются.
        assert!(set_op(&plan, CATEGORY, CHAT_MUTE_ROLE).is_some());
        assert!(set_op(&plan, CATEGORY, VOICE_MUTE_ROLE).is_some());
    }

    #[test]
    fn private_channels_keep_only_the_gate_for_non_staff() {
        let plan = plan(&snapshot(), &policy());
        // Клуб: разрешение писать перебивало бы чат-мут — остаётся только просмотр.
        assert_eq!(
            set_op(&plan, CLUB_CHAT, CLUB),
            Some((P_VIEW, Permissions::empty()))
        );
        assert!(plan.findings.iter().any(|f| {
            f.severity == Severity::Warning && f.text.contains("перебивало бы муты")
        }));
        // Администрации доверяем: оверрайт модераторов в их канале не меняется.
        assert_eq!(set_op(&plan, STAFF_CHAT, MOD), None);
        assert_eq!(set_op(&plan, STAFF_CHAT, ADMIN), None);
        // Персональный оверрайт не тронут.
        assert!(
            plan.findings
                .iter()
                .any(|f| f.text.contains("Персональные оверрайты"))
        );
    }

    #[test]
    fn verification_channel_opens_for_unverified_only() {
        let plan = plan(&snapshot(), &policy());
        let (allow, deny) = set_op(&plan, RULES, UNVERIFIED).expect("ворота неверифицированного");
        assert!(allow.contains(UNVERIFIED_GATE));
        assert!(deny.is_empty());
        assert_eq!(
            set_op(&plan, RULES, EVERYONE),
            None,
            "запрет просмотра уже стоит"
        );
        assert_eq!(
            set_op(&plan, RULES, MOD),
            None,
            "модераторы видят, как и раньше"
        );
    }

    #[test]
    fn read_only_channel_denies_write_for_everyone() {
        let plan = plan(&snapshot(), &policy());
        let news = permissions::applicable(ChannelType::News);
        assert_eq!(
            set_op(&plan, NEWS, EVERYONE),
            Some((Permissions::empty(), WRITE & news))
        );
        assert_eq!(
            set_op(&plan, NEWS, MOD),
            Some((WRITE & news, Permissions::empty()))
        );
    }

    #[test]
    fn plan_is_idempotent_and_restores_the_mute_invariant() {
        let before = snapshot();
        let policy = policy();
        let first = plan(&before, &policy);
        assert!(!first.is_empty());
        let after = simulate(&before, &first.ops);
        let second = plan(&after, &policy);
        assert!(
            second.is_empty(),
            "повторный план не пуст: {:#?}",
            second.ops
        );
        assert_eq!(second.classes, first.classes, "классы устойчивы");

        let staff: BTreeSet<RoleId> = after.hierarchy.staff().iter().map(|r| r.id).collect();
        assert!(
            !mute_invariant_holds(&before, &staff),
            "до: клуб перебивал мут"
        );
        assert!(mute_invariant_holds(&after, &staff));
        assert_ne!(first.fingerprint(), second.fingerprint());
        assert_eq!(first.fingerprint(), plan(&before, &policy).fingerprint());
    }

    #[test]
    fn effective_permissions_match_the_intent() {
        let before = snapshot();
        let policy = policy();
        let after = simulate(&before, &plan(&before, &policy).ops);
        let at = |roles: &[RoleId], channel: ChannelId| {
            effective(
                &after,
                UserId::new(7),
                roles,
                after.channel(channel).unwrap(),
            )
        };

        // Участник: видит общие каналы, не видит верификацию и штаб, читает новости.
        assert!(at(&[MEMBER], GENERAL).contains(P_VIEW | P_SEND));
        assert!(!at(&[MEMBER], RULES).view_channel());
        assert!(!at(&[MEMBER], STAFF_CHAT).view_channel());
        assert!(at(&[MEMBER], NEWS).view_channel());
        assert!(!at(&[MEMBER], NEWS).send_messages());
        // Неверифицированный: только канал верификации, без записи.
        assert!(at(&[UNVERIFIED], RULES).contains(P_VIEW | Permissions::READ_MESSAGE_HISTORY));
        assert!(!at(&[UNVERIFIED], RULES).send_messages());
        assert!(!at(&[UNVERIFIED], GENERAL).view_channel());
        assert!(at(&[], GENERAL).is_empty(), "без ролей — ничего");
        // Чат-мут действует везде, включая клуб; войс-мут — в голосе, слушать можно.
        assert!(!at(&[MEMBER, CHAT_MUTE_ROLE], GENERAL).send_messages());
        assert!(at(&[MEMBER, CHAT_MUTE_ROLE], GENERAL).view_channel());
        assert!(!at(&[MEMBER, CLUB, CHAT_MUTE_ROLE], CLUB_CHAT).send_messages());
        assert!(at(&[MEMBER, CLUB], CLUB_CHAT).send_messages());
        assert!(!at(&[MEMBER, VOICE_MUTE_ROLE], VOICE).speak());
        assert!(at(&[MEMBER, VOICE_MUTE_ROLE], VOICE).connect());
        // Модератор пишет в новости.
        assert!(at(&[MEMBER, MOD], NEWS).send_messages());
    }

    #[test]
    fn blockers_stop_the_plan() {
        let policy = policy();
        // Нет «Управлять ролями».
        let mut powerless = snapshot();
        powerless.hierarchy.set_permissions(BOT_ROLE, P_VIEW);
        let plan_without = plan(&powerless, &policy);
        assert!(plan_without.blocked());
        assert!(plan_without.is_empty());

        // Роль участника выше роли бота.
        let mut high = snapshot();
        let member = high.hierarchy.role(MEMBER).unwrap().clone();
        high.hierarchy = Hierarchy::new(
            GUILD,
            OWNER,
            high.hierarchy.roles().cloned().map(|r| {
                if r.id == MEMBER {
                    RoleInfo {
                        position: 50,
                        ..member.clone()
                    }
                } else {
                    r
                }
            }),
        );
        let plan_high = plan(&high, &policy);
        assert!(plan_high.blocked());
        assert!(plan_high.is_empty());

        // Роль политики — интеграция.
        let mut bad = policy.clone();
        bad.chat_mute = Some(BOT_ROLE);
        assert!(plan(&snapshot(), &bad).blocked());
        let mut twice = policy.clone();
        twice.voice_mute = Some(CHAT_MUTE_ROLE);
        assert!(plan(&snapshot(), &twice).blocked());
        let mut missing = policy;
        missing.unverified = Some(RoleId::new(999));
        assert!(plan(&snapshot(), &missing).blocked());
    }

    #[test]
    fn bot_limits_are_respected_per_channel() {
        let policy = policy();
        let mut snapshot = snapshot();
        // В голосовом канале у бота нет «Приоритетный режим» — этот бит пропускается.
        let voice = snapshot
            .channels
            .iter_mut()
            .find(|c| c.id == VOICE)
            .unwrap();
        voice.bot -= Permissions::PRIORITY_SPEAKER;
        // Клуб бот не видит — канал пропускается целиком.
        let club = snapshot
            .channels
            .iter_mut()
            .find(|c| c.id == CLUB_CHAT)
            .unwrap();
        club.bot = Permissions::empty();

        let plan = plan(&snapshot, &policy);
        let (_, deny) = set_op(&plan, VOICE, VOICE_MUTE_ROLE).unwrap();
        assert!(!deny.contains(Permissions::PRIORITY_SPEAKER));
        assert!(deny.contains(Permissions::SPEAK));
        assert!(ops_for(&plan, CLUB_CHAT).is_empty());
        assert!(plan.findings.iter().any(|f| f.text.contains("пропущены")));
        // Администратору доступно всё.
        let mut admin_bot = self::snapshot();
        admin_bot
            .hierarchy
            .set_permissions(BOT_ROLE, Permissions::ADMINISTRATOR);
        for channel in &mut admin_bot.channels {
            channel.bot = Permissions::empty();
        }
        assert!(!ops_for(&self::plan(&admin_bot, &policy), CLUB_CHAT).is_empty());
    }

    #[test]
    fn explicit_classes_override_inference() {
        let mut policy = policy();
        // Клуб становится открытым: запрет просмотра снимается, оверрайт клуба — лишний.
        policy.channels.insert(CLUB_CHAT, ChannelClass::Public);
        // Новости становятся закрытыми для модераторов.
        policy.channels.insert(
            NEWS,
            ChannelClass::Private {
                roles: BTreeSet::from([MOD]),
            },
        );
        let before = snapshot();
        let plan = plan(&before, &policy);
        assert!(deletes(&plan, CLUB_CHAT, EVERYONE));
        // Просмотр для клуба остаётся (безвреден и ничего не меняет), запись — снимается.
        assert_eq!(
            set_op(&plan, CLUB_CHAT, CLUB),
            Some((P_VIEW, Permissions::empty()))
        );
        let news = permissions::applicable(ChannelType::News);
        assert_eq!(
            set_op(&plan, NEWS, EVERYONE),
            Some((Permissions::empty(), P_VIEW | (P_SEND & news)))
        );
        assert_eq!(
            set_op(&plan, NEWS, MOD),
            Some((P_VIEW | P_SEND, Permissions::empty()))
        );
        let after = simulate(&before, &plan.ops);
        assert!(self::plan(&after, &policy).is_empty());
    }

    #[test]
    fn member_denies_migrate_to_everyone() {
        let policy = policy();
        let mut snapshot = snapshot();
        let general = snapshot
            .channels
            .iter_mut()
            .find(|c| c.id == GENERAL)
            .unwrap();
        general.overwrites = vec![overwrite(
            MEMBER,
            Permissions::empty(),
            Permissions::ATTACH_FILES,
        )];
        let plan = plan(&snapshot, &policy);
        assert_eq!(
            set_op(&plan, GENERAL, EVERYONE),
            Some((Permissions::empty(), Permissions::ATTACH_FILES))
        );
        assert!(deletes(&plan, GENERAL, MEMBER));
        let after = simulate(&snapshot, &plan.ops);
        assert!(self::plan(&after, &policy).is_empty());
    }

    #[test]
    fn class_keys_round_trip() {
        for key in ChannelClass::KEYS {
            let class = ChannelClass::from_key(key, BTreeSet::new()).unwrap();
            assert_eq!(class.key(), key);
        }
        assert!(ChannelClass::from_key("secret", BTreeSet::new()).is_none());
        assert!(ChannelClass::takes_roles("private"));
        assert!(!ChannelClass::takes_roles("public"));
        for purpose in PolicyRole::ALL {
            assert_eq!(PolicyRole::from_key(purpose.key()), Some(purpose));
        }
        let mut policy = Policy::new(MEMBER);
        assert!(!policy.set_role(PolicyRole::Member, None));
        assert!(policy.set_role(PolicyRole::ChatMute, Some(CHAT_MUTE_ROLE)));
        assert_eq!(policy.role(PolicyRole::ChatMute), Some(CHAT_MUTE_ROLE));
    }

    #[test]
    fn verification_passes_after_the_plan_and_fails_before() {
        let before = snapshot();
        let policy = policy();
        let plan = plan(&before, &policy);
        let after = simulate(&before, &plan.ops);

        let checks = verify(&after, &policy, &plan.classes);
        assert_eq!(checks.len(), 5);
        for check in &checks {
            assert!(check.ok(), "{}: {:?}", check.title, check.failing);
        }

        let broken = verify(&before, &policy, &plan.classes);
        let by_title = |title: &str| {
            broken
                .iter()
                .find(|check| check.title.starts_with(title))
                .unwrap()
        };
        // @everyone видел общие каналы, чат-мут ничего не запрещал, неверифицированный видел
        // всё. Войс-мут проверяется только там, где участник может подключиться, а «до» права
        // голоса не было ни у кого — проверка проходит вхолостую.
        assert!(by_title("Без ролей").failing.contains(&GENERAL));
        assert!(by_title("Чат-мут").failing.contains(&GENERAL));
        assert!(by_title("Неверифицированный").failing.contains(&GENERAL));
        assert!(by_title("Войс-мут").ok());
    }
}
