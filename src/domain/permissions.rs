//! Каталог прав Discord по состоянию на октябрь 2026 года. Чистые данные — покрыты тестами.
//!
//! Источник — таблица «Bitwise Permission Flags» документации Discord: 52 права, биты 0–52 без
//! бита 47 (не используется). Для каждого права известны русское название (как в клиенте
//! Discord), раздел настроек, типы каналов, в оверрайтах которых оно имеет смысл, и
//! [`Grade`] — уровень полномочий, по которому классифицируются роли (см. [`crate::domain::roles`]).
//!
//! **Ограничение serenity 0.12.** Библиотека знает биты только до 50 (`USE_EXTERNAL_APPS`), а
//! при чтении ответов Discord отбрасывает неизвестные биты (`from_bits_truncate`). Поэтому права
//! [`PIN_MESSAGES`] и [`BYPASS_SLOWMODE`] бот не может **увидеть** у ролей и в оверрайтах, но
//! может **выдать** — при отправке уходят все биты. Сравнивать текущее состояние с целевым
//! можно только по [`OBSERVABLE`] битам, иначе план никогда не станет пустым.

use serenity::all::{ChannelType, Permissions};

/// «Закреплять сообщения», бит 51 (с 2025 года; serenity 0.12 его не знает).
pub const PIN_MESSAGES: Permissions = Permissions::from_bits_retain(1 << 51);
/// «Обходить медленный режим», бит 52 (с 2025 года; serenity 0.12 его не знает).
pub const BYPASS_SLOWMODE: Permissions = Permissions::from_bits_retain(1 << 52);

/// Биты, которые serenity читает из ответов Discord. Только их можно сравнивать с реальностью.
pub const OBSERVABLE: Permissions = Permissions::all();

/// Раздел настроек роли в клиенте Discord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    General,
    Membership,
    Text,
    Voice,
    Apps,
    Stage,
    Events,
    Advanced,
}

impl Category {
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::Membership,
        Self::Text,
        Self::Voice,
        Self::Apps,
        Self::Stage,
        Self::Events,
        Self::Advanced,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "Основные права сервера",
            Self::Membership => "Права участников",
            Self::Text => "Права текстовых каналов",
            Self::Voice => "Права голосовых каналов",
            Self::Apps => "Права приложений",
            Self::Stage => "Права трибун",
            Self::Events => "Права мероприятий",
            Self::Advanced => "Расширенные права",
        }
    }
}

/// Типы каналов, в оверрайтах которых право имеет смысл: текстовые (T), голосовые (V), трибуны
/// (S). Пустой набор — право действует только на уровне сервера.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub text: bool,
    pub voice: bool,
    pub stage: bool,
}

impl Scope {
    pub const NONE: Self = Self::new(false, false, false);
    pub const T: Self = Self::new(true, false, false);
    pub const V: Self = Self::new(false, true, false);
    pub const S: Self = Self::new(false, false, true);
    pub const TV: Self = Self::new(true, true, false);
    pub const VS: Self = Self::new(false, true, true);
    pub const TVS: Self = Self::new(true, true, true);

    const fn new(text: bool, voice: bool, stage: bool) -> Self {
        Self { text, voice, stage }
    }

    pub const fn is_channel(self) -> bool {
        self.text || self.voice || self.stage
    }

    /// «T V S» — как в документации Discord; «сервер» — для прав без оверрайтов.
    pub fn label(self) -> String {
        if !self.is_channel() {
            return "сервер".to_string();
        }
        [(self.text, "T"), (self.voice, "V"), (self.stage, "S")]
            .into_iter()
            .filter(|(present, _)| *present)
            .map(|(_, letter)| letter)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Имеет ли право смысл в канале данного типа.
    pub const fn covers(self, kind: ChannelType) -> bool {
        match kind {
            ChannelType::Text
            | ChannelType::News
            | ChannelType::Forum
            | ChannelType::NewsThread
            | ChannelType::PublicThread
            | ChannelType::PrivateThread => self.text,
            ChannelType::Voice => self.voice,
            ChannelType::Stage => self.stage,
            // Категория наследуется каналами любого типа.
            ChannelType::Category => self.is_channel(),
            // Прочие и неизвестные типы (например, медиаканалы, которых serenity 0.12 не знает)
            // считаются текстовыми, чтобы не потерять права.
            _ => self.text,
        }
    }
}

/// Уровень полномочий. Порядок вариантов — порядок возрастания опасности.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grade {
    /// Обычное участие: писать, говорить, реагировать.
    Everyday,
    /// Доверенные участники: уведомлять всех, обходить медленный режим, создавать мероприятия.
    Trusted,
    /// Модерация: наказывать участников и править чужой контент.
    Moderation,
    /// Управление сервером: роли, каналы, вебхуки, журнал аудита.
    Management,
    /// Все права и обход любых оверрайтов.
    Administrator,
}

impl Grade {
    #[cfg(test)]
    pub const ALL: [Self; 5] = [
        Self::Everyday,
        Self::Trusted,
        Self::Moderation,
        Self::Management,
        Self::Administrator,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Everyday => "Обычные",
            Self::Trusted => "Доверенные",
            Self::Moderation => "Модерация",
            Self::Management => "Управление",
            Self::Administrator => "Администратор",
        }
    }

    const fn rank(self) -> u8 {
        match self {
            Self::Everyday => 0,
            Self::Trusted => 1,
            Self::Moderation => 2,
            Self::Management => 3,
            Self::Administrator => 4,
        }
    }
}

/// Описание одного права.
#[derive(Debug)]
pub struct Spec {
    pub flag: Permissions,
    /// Имя в документации Discord.
    pub key: &'static str,
    /// Название в русском клиенте Discord.
    pub name: &'static str,
    pub category: Category,
    pub scope: Scope,
    pub grade: Grade,
}

const fn spec(
    flag: Permissions,
    key: &'static str,
    name: &'static str,
    category: Category,
    scope: Scope,
    grade: Grade,
) -> Spec {
    Spec {
        flag,
        key,
        name,
        category,
        scope,
        grade,
    }
}

use Category as C;
use Grade as G;
use Permissions as P;

/// Все права по возрастанию бита.
pub const CATALOG: &[Spec] = &[
    spec(
        P::CREATE_INSTANT_INVITE,
        "CREATE_INSTANT_INVITE",
        "Создание приглашения",
        C::Membership,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::KICK_MEMBERS,
        "KICK_MEMBERS",
        "Выгонять участников",
        C::Membership,
        Scope::NONE,
        G::Moderation,
    ),
    spec(
        P::BAN_MEMBERS,
        "BAN_MEMBERS",
        "Банить участников",
        C::Membership,
        Scope::NONE,
        G::Moderation,
    ),
    spec(
        P::ADMINISTRATOR,
        "ADMINISTRATOR",
        "Администратор",
        C::Advanced,
        Scope::NONE,
        G::Administrator,
    ),
    spec(
        P::MANAGE_CHANNELS,
        "MANAGE_CHANNELS",
        "Управлять каналами",
        C::General,
        Scope::TVS,
        G::Management,
    ),
    spec(
        P::MANAGE_GUILD,
        "MANAGE_GUILD",
        "Управлять сервером",
        C::General,
        Scope::NONE,
        G::Management,
    ),
    spec(
        P::ADD_REACTIONS,
        "ADD_REACTIONS",
        "Добавлять реакции",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::VIEW_AUDIT_LOG,
        "VIEW_AUDIT_LOG",
        "Просматривать журнал аудита",
        C::General,
        Scope::NONE,
        G::Management,
    ),
    spec(
        P::PRIORITY_SPEAKER,
        "PRIORITY_SPEAKER",
        "Приоритетный режим",
        C::Voice,
        Scope::V,
        G::Trusted,
    ),
    spec(
        P::STREAM,
        "STREAM",
        "Видео",
        C::Voice,
        Scope::VS,
        G::Everyday,
    ),
    spec(
        P::VIEW_CHANNEL,
        "VIEW_CHANNEL",
        "Просматривать каналы",
        C::General,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::SEND_MESSAGES,
        "SEND_MESSAGES",
        "Отправлять сообщения",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::SEND_TTS_MESSAGES,
        "SEND_TTS_MESSAGES",
        "Отправлять сообщения TTS",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::MANAGE_MESSAGES,
        "MANAGE_MESSAGES",
        "Управлять сообщениями",
        C::Text,
        Scope::TVS,
        G::Moderation,
    ),
    spec(
        P::EMBED_LINKS,
        "EMBED_LINKS",
        "Встраивать ссылки",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::ATTACH_FILES,
        "ATTACH_FILES",
        "Прикреплять файлы",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::READ_MESSAGE_HISTORY,
        "READ_MESSAGE_HISTORY",
        "Читать историю сообщений",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::MENTION_EVERYONE,
        "MENTION_EVERYONE",
        "Упоминание @everyone, @here и всех ролей",
        C::Text,
        Scope::TVS,
        G::Trusted,
    ),
    spec(
        P::USE_EXTERNAL_EMOJIS,
        "USE_EXTERNAL_EMOJIS",
        "Использовать внешние эмодзи",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::VIEW_GUILD_INSIGHTS,
        "VIEW_GUILD_INSIGHTS",
        "Просматривать аналитику сервера",
        C::General,
        Scope::NONE,
        G::Trusted,
    ),
    spec(
        P::CONNECT,
        "CONNECT",
        "Подключаться",
        C::Voice,
        Scope::VS,
        G::Everyday,
    ),
    spec(
        P::SPEAK,
        "SPEAK",
        "Говорить",
        C::Voice,
        Scope::V,
        G::Everyday,
    ),
    spec(
        P::MUTE_MEMBERS,
        "MUTE_MEMBERS",
        "Отключать участникам микрофон",
        C::Voice,
        Scope::VS,
        G::Moderation,
    ),
    spec(
        P::DEAFEN_MEMBERS,
        "DEAFEN_MEMBERS",
        "Отключать участникам звук",
        C::Voice,
        Scope::V,
        G::Moderation,
    ),
    spec(
        P::MOVE_MEMBERS,
        "MOVE_MEMBERS",
        "Перемещать участников",
        C::Voice,
        Scope::VS,
        G::Moderation,
    ),
    spec(
        P::USE_VAD,
        "USE_VAD",
        "Использовать режим активации по голосу",
        C::Voice,
        Scope::V,
        G::Everyday,
    ),
    spec(
        P::CHANGE_NICKNAME,
        "CHANGE_NICKNAME",
        "Изменить никнейм",
        C::Membership,
        Scope::NONE,
        G::Everyday,
    ),
    spec(
        P::MANAGE_NICKNAMES,
        "MANAGE_NICKNAMES",
        "Управлять никнеймами",
        C::Membership,
        Scope::NONE,
        G::Moderation,
    ),
    spec(
        P::MANAGE_ROLES,
        "MANAGE_ROLES",
        "Управлять ролями",
        C::General,
        Scope::TVS,
        G::Management,
    ),
    spec(
        P::MANAGE_WEBHOOKS,
        "MANAGE_WEBHOOKS",
        "Управлять вебхуками",
        C::General,
        Scope::TVS,
        G::Management,
    ),
    spec(
        P::MANAGE_GUILD_EXPRESSIONS,
        "MANAGE_GUILD_EXPRESSIONS",
        "Управлять выражениями",
        C::General,
        Scope::NONE,
        G::Management,
    ),
    spec(
        P::USE_APPLICATION_COMMANDS,
        "USE_APPLICATION_COMMANDS",
        "Использовать команды приложений",
        C::Apps,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::REQUEST_TO_SPEAK,
        "REQUEST_TO_SPEAK",
        "Запрос на выступление",
        C::Stage,
        Scope::S,
        G::Everyday,
    ),
    spec(
        P::MANAGE_EVENTS,
        "MANAGE_EVENTS",
        "Управлять мероприятиями",
        C::Events,
        Scope::VS,
        G::Moderation,
    ),
    spec(
        P::MANAGE_THREADS,
        "MANAGE_THREADS",
        "Управлять ветками",
        C::Text,
        Scope::T,
        G::Moderation,
    ),
    spec(
        P::CREATE_PUBLIC_THREADS,
        "CREATE_PUBLIC_THREADS",
        "Создавать публичные ветки",
        C::Text,
        Scope::T,
        G::Everyday,
    ),
    spec(
        P::CREATE_PRIVATE_THREADS,
        "CREATE_PRIVATE_THREADS",
        "Создавать приватные ветки",
        C::Text,
        Scope::T,
        G::Everyday,
    ),
    spec(
        P::USE_EXTERNAL_STICKERS,
        "USE_EXTERNAL_STICKERS",
        "Использовать внешние стикеры",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::SEND_MESSAGES_IN_THREADS,
        "SEND_MESSAGES_IN_THREADS",
        "Отправлять сообщения в ветках",
        C::Text,
        Scope::T,
        G::Everyday,
    ),
    spec(
        P::USE_EMBEDDED_ACTIVITIES,
        "USE_EMBEDDED_ACTIVITIES",
        "Использовать активности",
        C::Apps,
        Scope::TV,
        G::Everyday,
    ),
    spec(
        P::MODERATE_MEMBERS,
        "MODERATE_MEMBERS",
        "Тайм-аут участников",
        C::Membership,
        Scope::NONE,
        G::Moderation,
    ),
    spec(
        P::VIEW_CREATOR_MONETIZATION_ANALYTICS,
        "VIEW_CREATOR_MONETIZATION_ANALYTICS",
        "Просматривать аналитику монетизации",
        C::General,
        Scope::NONE,
        G::Trusted,
    ),
    spec(
        P::USE_SOUNDBOARD,
        "USE_SOUNDBOARD",
        "Использовать звуковую панель",
        C::Voice,
        Scope::V,
        G::Everyday,
    ),
    spec(
        P::CREATE_GUILD_EXPRESSIONS,
        "CREATE_GUILD_EXPRESSIONS",
        "Создавать выражения",
        C::General,
        Scope::NONE,
        G::Trusted,
    ),
    spec(
        P::CREATE_EVENTS,
        "CREATE_EVENTS",
        "Создавать мероприятия",
        C::Events,
        Scope::VS,
        G::Trusted,
    ),
    spec(
        P::USE_EXTERNAL_SOUNDS,
        "USE_EXTERNAL_SOUNDS",
        "Использовать внешние звуки",
        C::Voice,
        Scope::V,
        G::Everyday,
    ),
    spec(
        P::SEND_VOICE_MESSAGES,
        "SEND_VOICE_MESSAGES",
        "Отправлять голосовые сообщения",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::SET_VOICE_CHANNEL_STATUS,
        "SET_VOICE_CHANNEL_STATUS",
        "Задавать статус голосового канала",
        C::Voice,
        Scope::V,
        G::Everyday,
    ),
    spec(
        P::SEND_POLLS,
        "SEND_POLLS",
        "Создавать опросы",
        C::Text,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        P::USE_EXTERNAL_APPS,
        "USE_EXTERNAL_APPS",
        "Использовать внешние приложения",
        C::Apps,
        Scope::TVS,
        G::Everyday,
    ),
    spec(
        PIN_MESSAGES,
        "PIN_MESSAGES",
        "Закреплять сообщения",
        C::Text,
        Scope::T,
        G::Moderation,
    ),
    spec(
        BYPASS_SLOWMODE,
        "BYPASS_SLOWMODE",
        "Обходить медленный режим",
        C::Text,
        Scope::TVS,
        G::Trusted,
    ),
];

/// Все права каталога, включая неизвестные serenity.
pub const KNOWN: Permissions = at_least(Grade::Everyday);

/// Права, имеющие смысл в оверрайтах каналов (хотя бы одного типа).
pub const CHANNEL_SCOPED: Permissions = channel_scoped();

/// Права уровня не ниже «Модерация»: роль с любым из них — административная.
pub const STAFF: Permissions = at_least(Grade::Moderation);

const fn channel_scoped() -> Permissions {
    let mut acc = Permissions::empty();
    let mut i = 0;
    while i < CATALOG.len() {
        if CATALOG[i].scope.is_channel() {
            acc = acc.union(CATALOG[i].flag);
        }
        i += 1;
    }
    acc
}

/// Права с уровнем ровно `grade`.
#[cfg(test)]
pub const fn exactly(grade: Grade) -> Permissions {
    let mut acc = Permissions::empty();
    let mut i = 0;
    while i < CATALOG.len() {
        if CATALOG[i].grade.rank() == grade.rank() {
            acc = acc.union(CATALOG[i].flag);
        }
        i += 1;
    }
    acc
}

/// Права с уровнем не ниже `grade`.
pub const fn at_least(grade: Grade) -> Permissions {
    let mut acc = Permissions::empty();
    let mut i = 0;
    while i < CATALOG.len() {
        if CATALOG[i].grade.rank() >= grade.rank() {
            acc = acc.union(CATALOG[i].flag);
        }
        i += 1;
    }
    acc
}

/// Высший уровень среди прав набора; у пустого набора — [`Grade::Everyday`].
pub fn grade(permissions: Permissions) -> Grade {
    CATALOG
        .iter()
        .filter(|spec| permissions.contains(spec.flag))
        .map(|spec| spec.grade)
        .max()
        .unwrap_or(Grade::Everyday)
}

/// Права, имеющие смысл в оверрайтах канала данного типа.
pub fn applicable(kind: ChannelType) -> Permissions {
    CATALOG
        .iter()
        .filter(|spec| spec.scope.covers(kind))
        .fold(Permissions::empty(), |acc, spec| acc | spec.flag)
}

/// Что Discord запрещает неявно вместе с запретом `denied`: без «Просматривать каналы» не
/// действует ни одно право канала, без «Отправлять сообщения» — упоминания, TTS, файлы и ссылки.
pub fn implied_denies(denied: Permissions) -> Permissions {
    let mut implied = denied;
    if denied.view_channel() {
        implied |= CHANNEL_SCOPED;
    }
    if denied.send_messages() {
        implied |= Permissions::MENTION_EVERYONE
            | Permissions::SEND_TTS_MESSAGES
            | Permissions::ATTACH_FILES
            | Permissions::EMBED_LINKS;
    }
    implied
}

/// Русские названия прав в порядке битов: «Управлять сервером», «Встраивать ссылки».
/// Биты вне каталога — «бит N».
pub fn describe(permissions: Permissions) -> String {
    names(permissions)
        .into_iter()
        .map(|name| format!("«{name}»"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Названия прав в порядке битов; для битов вне каталога — «бит N».
pub fn names(permissions: Permissions) -> Vec<String> {
    let mut out: Vec<String> = CATALOG
        .iter()
        .filter(|spec| permissions.contains(spec.flag))
        .map(|spec| spec.name.to_string())
        .collect();
    let unknown = permissions.difference(KNOWN).bits();
    out.extend(
        (0..u64::BITS)
            .filter(|bit| unknown & (1 << bit) != 0)
            .map(|bit| format!("бит {bit}")),
    );
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_covers_every_documented_bit_once() {
        // Биты 0–52 без 47 (не используется Discord).
        let expected: Vec<u64> = (0..=52).filter(|&bit| bit != 47).collect();
        let actual: Vec<u64> = CATALOG
            .iter()
            .map(|spec| spec.flag.bits().trailing_zeros().into())
            .collect();
        assert_eq!(actual, expected, "каталог упорядочен по битам и полон");
        for spec in CATALOG {
            assert_eq!(spec.flag.bits().count_ones(), 1, "{}: один бит", spec.key);
        }
        let names: HashSet<&str> = CATALOG.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), CATALOG.len(), "названия уникальны");
        let keys: HashSet<&str> = CATALOG.iter().map(|spec| spec.key).collect();
        assert_eq!(keys.len(), CATALOG.len(), "ключи уникальны");
    }

    #[test]
    fn known_extends_serenity_by_two_bits() {
        assert_eq!(KNOWN, Permissions::all() | PIN_MESSAGES | BYPASS_SLOWMODE);
        assert!(!OBSERVABLE.contains(PIN_MESSAGES));
        assert!(!OBSERVABLE.contains(BYPASS_SLOWMODE));
    }

    #[test]
    fn grades_partition_the_catalog() {
        let mut total = Permissions::empty();
        for grade in Grade::ALL {
            let set = exactly(grade);
            assert!(total.intersection(set).is_empty(), "{grade:?} пересекается");
            total |= set;
        }
        assert_eq!(total, KNOWN);
        assert_eq!(at_least(Grade::Administrator), Permissions::ADMINISTRATOR);
        assert!(STAFF.contains(Permissions::BAN_MEMBERS | Permissions::MANAGE_ROLES));
        assert!(!STAFF.contains(Permissions::MENTION_EVERYONE));
        assert!(!STAFF.contains(Permissions::SEND_MESSAGES));
        assert_eq!(
            grade(Permissions::SEND_MESSAGES | Permissions::KICK_MEMBERS),
            Grade::Moderation
        );
        assert_eq!(grade(Permissions::empty()), Grade::Everyday);
    }

    #[test]
    fn scopes_follow_the_channel_types_column() {
        let text = applicable(ChannelType::Text);
        assert!(text.contains(Permissions::SEND_MESSAGES | Permissions::MANAGE_THREADS));
        assert!(!text.contains(Permissions::CONNECT));
        assert!(!text.contains(Permissions::KICK_MEMBERS));

        let voice = applicable(ChannelType::Voice);
        assert!(voice.contains(Permissions::SPEAK | Permissions::SEND_MESSAGES));
        assert!(!voice.contains(Permissions::REQUEST_TO_SPEAK));
        assert!(!voice.contains(Permissions::CREATE_PUBLIC_THREADS));

        let stage = applicable(ChannelType::Stage);
        assert!(stage.contains(Permissions::REQUEST_TO_SPEAK | Permissions::CONNECT));
        assert!(!stage.contains(Permissions::SPEAK));

        assert_eq!(applicable(ChannelType::Category), CHANNEL_SCOPED);
        assert!(!CHANNEL_SCOPED.contains(Permissions::ADMINISTRATOR));
    }

    #[test]
    fn implied_denies_follow_discord_rules() {
        assert!(implied_denies(Permissions::VIEW_CHANNEL).contains(CHANNEL_SCOPED));
        let send = implied_denies(Permissions::SEND_MESSAGES);
        assert!(send.contains(Permissions::ATTACH_FILES | Permissions::MENTION_EVERYONE));
        assert!(!send.contains(Permissions::ADD_REACTIONS));
        assert_eq!(implied_denies(Permissions::CONNECT), Permissions::CONNECT);
    }

    #[test]
    fn describes_in_russian_with_unknown_bits() {
        assert_eq!(
            describe(Permissions::EMBED_LINKS | Permissions::MANAGE_GUILD),
            "«Управлять сервером», «Встраивать ссылки»"
        );
        assert_eq!(describe(PIN_MESSAGES), "«Закреплять сообщения»");
        assert_eq!(describe(Permissions::from_bits_retain(1 << 60)), "«бит 60»");
        assert_eq!(describe(Permissions::empty()), "");
    }
}
