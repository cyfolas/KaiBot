//! Классификация ролей сервера. Чистая логика — покрыта тестами.
//!
//! Роль описывается двумя независимыми признаками:
//! * [`Tier`] — уровень полномочий по правам роли (высший [`Grade`] среди её прав);
//! * [`Origin`] — происхождение: обычная роль, @everyone или роль, которой управляет Discord
//!   (интеграция, бусты, подписки, связанные роли) — такие роли нельзя выдавать вручную.
//!
//! Административной считается обычная роль уровня не ниже «Модерация». Боту не нужны списки
//! ролей в настройках: классификация вычисляется из прав, которые сервер уже выставил.

use serenity::all::{Permissions, Role, RoleTags};

use super::permissions::{self, Grade};

/// Уровень полномочий роли. Порядок вариантов — от старшего к младшему.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Есть «Администратор».
    Administrator,
    /// Управляет сервером: роли, каналы, вебхуки, журнал аудита, выражения.
    Management,
    /// Наказывает участников и правит чужой контент.
    Moderation,
    /// Доверенные участники: @everyone, обход медленного режима, мероприятия.
    Trusted,
    /// Обычные права: писать, говорить, реагировать.
    Member,
    /// Прав нет: цвет, разделитель, метка.
    Cosmetic,
}

impl Tier {
    pub fn of(permissions: Permissions) -> Self {
        if permissions.is_empty() {
            return Self::Cosmetic;
        }
        match permissions::grade(permissions) {
            Grade::Administrator => Self::Administrator,
            Grade::Management => Self::Management,
            Grade::Moderation => Self::Moderation,
            Grade::Trusted => Self::Trusted,
            Grade::Everyday => Self::Member,
        }
    }

    /// Уровни, составляющие администрацию сервера.
    pub fn is_staff(self) -> bool {
        matches!(
            self,
            Self::Administrator | Self::Management | Self::Moderation
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Administrator => "Администраторы",
            Self::Management => "Управление",
            Self::Moderation => "Модерация",
            Self::Trusted => "Доверенные",
            Self::Member => "Участники",
            Self::Cosmetic => "Без прав",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Self::Administrator => "👑",
            Self::Management => "🛡️",
            Self::Moderation => "⚖️",
            Self::Trusted => "⭐",
            Self::Member => "👥",
            Self::Cosmetic => "🎨",
        }
    }
}

/// Происхождение роли.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Обычная роль, созданная администрацией.
    #[default]
    Regular,
    /// @everyone: есть у всех, ID совпадает с ID сервера.
    Everyone,
    /// Роль бота или другой интеграции.
    Integration,
    /// Роль бустеров сервера.
    Booster,
    /// Роль платной подписки сервера.
    Subscription,
    /// Связанная роль (по подключённым аккаунтам).
    Linked,
}

impl Origin {
    pub fn of(role: &Role) -> Self {
        Self::from_parts(
            role.id.get() == role.guild_id.get(),
            role.managed,
            &role.tags,
        )
    }

    pub fn from_parts(everyone: bool, managed: bool, tags: &RoleTags) -> Self {
        if everyone {
            Self::Everyone
        } else if tags.premium_subscriber {
            Self::Booster
        } else if tags.subscription_listing_id.is_some() {
            Self::Subscription
        } else if tags.guild_connections {
            Self::Linked
        } else if managed || tags.bot_id.is_some() || tags.integration_id.is_some() {
            Self::Integration
        } else {
            Self::Regular
        }
    }

    /// Discord не даёт выдавать и снимать такую роль вручную.
    pub fn is_managed(self) -> bool {
        !matches!(self, Self::Regular)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Regular => "обычная",
            Self::Everyone => "@everyone",
            Self::Integration => "интеграция",
            Self::Booster => "бусты",
            Self::Subscription => "подписка",
            Self::Linked => "связанная",
        }
    }
}

/// Итог классификации.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Class {
    pub tier: Tier,
    pub origin: Origin,
}

impl Class {
    pub fn from_parts(origin: Origin, permissions: Permissions) -> Self {
        Self {
            tier: Tier::of(permissions),
            origin,
        }
    }

    /// Административная роль: обычная, уровня не ниже «Модерация».
    pub fn is_staff(self) -> bool {
        self.origin == Origin::Regular && self.tier.is_staff()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_is_the_highest_grade_present() {
        assert_eq!(Tier::of(Permissions::empty()), Tier::Cosmetic);
        assert_eq!(Tier::of(Permissions::SEND_MESSAGES), Tier::Member);
        assert_eq!(Tier::of(Permissions::MENTION_EVERYONE), Tier::Trusted);
        assert_eq!(
            Tier::of(Permissions::SEND_MESSAGES | Permissions::MANAGE_MESSAGES),
            Tier::Moderation
        );
        assert_eq!(Tier::of(Permissions::MANAGE_ROLES), Tier::Management);
        assert_eq!(
            Tier::of(Permissions::ADMINISTRATOR | Permissions::KICK_MEMBERS),
            Tier::Administrator
        );
        assert!(Tier::Moderation.is_staff());
        assert!(!Tier::Trusted.is_staff());
        assert!(Tier::Administrator < Tier::Cosmetic, "старшие раньше");
    }

    /// `RoleTags` — `#[non_exhaustive]`, поэтому строится из JSON, как приходит от Discord
    /// (флаги-«присутствия» передаются как `null`).
    fn tags(json: &str) -> RoleTags {
        serenity::json::from_str(json).unwrap()
    }

    #[test]
    fn origin_from_tags() {
        let plain = tags("{}");
        assert_eq!(Origin::from_parts(true, false, &plain), Origin::Everyone);
        assert_eq!(Origin::from_parts(false, false, &plain), Origin::Regular);
        assert_eq!(Origin::from_parts(false, true, &plain), Origin::Integration);
        let bot = tags(r#"{"bot_id": "42"}"#);
        assert_eq!(Origin::from_parts(false, true, &bot), Origin::Integration);
        let booster = tags(r#"{"premium_subscriber": null}"#);
        assert_eq!(Origin::from_parts(false, true, &booster), Origin::Booster);
        let linked = tags(r#"{"guild_connections": null}"#);
        assert_eq!(Origin::from_parts(false, true, &linked), Origin::Linked);
        let subscription = tags(r#"{"subscription_listing_id": "7"}"#);
        assert_eq!(
            Origin::from_parts(false, true, &subscription),
            Origin::Subscription
        );
        assert!(Origin::Booster.is_managed());
        assert!(!Origin::Regular.is_managed());
    }

    #[test]
    fn staff_requires_regular_origin_and_staff_tier() {
        let moderator = Class::from_parts(Origin::Regular, Permissions::BAN_MEMBERS);
        assert!(moderator.is_staff());
        let bot = Class::from_parts(Origin::Integration, Permissions::ADMINISTRATOR);
        assert!(!bot.is_staff());
        let trusted = Class::from_parts(Origin::Regular, Permissions::MENTION_EVERYONE);
        assert!(!trusted.is_staff());
    }
}
