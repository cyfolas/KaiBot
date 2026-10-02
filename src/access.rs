//! Режимы доступа к боту. Чистая логика без обращений к Discord — покрыта тестами.
//!
//! Доступ к боту — отдельный слой поверх прав Discord: право команды
//! ([`crate::framework::SlashCommand::permission`]) проверяется всегда, а здесь решается, может
//! ли участник вообще пользоваться ботом. Уровни проверяются по порядку, первый отказ — итог:
//!
//! 1. **Разработчики бота** проходят всегда: так они не запрут сами себя ни режимом, ни ошибкой
//!    в правилах. Прав на сервере это не даёт.
//! 2. **Команды разработчиков** — только разработчикам.
//! 3. **Режим приложения** (управляют разработчики): `public` или `dev-only` (обслуживание).
//! 4. **Глобальная блокировка** пользователя разработчиками — на всех серверах.
//! 5. **Управляющие ботом на сервере** (администратор, «Управлять сервером», владелец) проходят:
//!    кто может изменить правила, тот им не подчиняется — иначе администрация могла бы запереть
//!    сама себя.
//! 6. **Режим сервера**: `locked` — только управляющие (п. 5).
//! 7. **Правила сервера**: самое конкретное правило решает — правило пользователя важнее правил
//!    его ролей (как оверрайты каналов в Discord); среди ролей запрет важнее разрешения.
//! 8. **Режим сервера** по умолчанию: `open` — разрешено, `restricted` — только по правилам.

use std::collections::{BTreeMap, BTreeSet};

use serenity::all::{Mentionable, Permissions, RoleId, UserId};

/// Права, дающие управление ботом на сервере. Владелец сервера получает от Discord все права,
/// поэтому отдельно не проверяется.
pub const MANAGER_PERMISSIONS: Permissions =
    Permissions::ADMINISTRATOR.union(Permissions::MANAGE_GUILD);

/// Предел числа правил на сервере: ограничивает объём хранения и размер `/access status`.
pub const MAX_RULES: usize = 100;

/// Режим приложения.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GlobalMode {
    #[default]
    Public,
    /// Обслуживание: ботом пользуются только разработчики.
    DevOnly,
}

impl GlobalMode {
    pub const ALL: [Self; 2] = [Self::Public, Self::DevOnly];

    /// Значение в базе и в опциях команд.
    pub fn key(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::DevOnly => "dev-only",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "Публичный",
            Self::DevOnly => "Только разработчики",
        }
    }
}

/// Доступ на уровне приложения: режим и заблокированные пользователи.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalAccess {
    pub mode: GlobalMode,
    pub blocked: BTreeSet<UserId>,
}

/// Режим сервера.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuildMode {
    /// Все, кроме запрещённых правилами.
    #[default]
    Open,
    /// Только те, кому доступ выдан правилами.
    Restricted,
    /// Только управляющие ботом на сервере; правила не действуют, но сохраняются.
    Locked,
}

impl GuildMode {
    pub const ALL: [Self; 3] = [Self::Open, Self::Restricted, Self::Locked];

    pub fn key(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Restricted => "restricted",
            Self::Locked => "locked",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Открытый",
            Self::Restricted => "По списку",
            Self::Locked => "Только администрация",
        }
    }

    pub fn explain(self) -> &'static str {
        match self {
            Self::Open => "ботом пользуются все, кроме тех, кому доступ закрыт",
            Self::Restricted => "ботом пользуются только те, кому доступ выдан",
            Self::Locked => "ботом пользуется только администрация сервера",
        }
    }
}

/// К кому относится правило.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    User(UserId),
    Role(RoleId),
}

impl Subject {
    pub fn mention(self) -> String {
        match self {
            Self::User(id) => id.mention().to_string(),
            Self::Role(id) => id.mention().to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Allow,
    Deny,
}

impl Effect {
    pub fn key(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [Self::Allow, Self::Deny]
            .into_iter()
            .find(|effect| effect.key() == key)
    }
}

/// Доступ на уровне сервера: режим и правила (не больше одного на пользователя или роль).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildAccess {
    pub mode: GuildMode,
    rules: BTreeMap<Subject, Effect>,
}

/// Достигнут [`MAX_RULES`].
#[derive(Debug, PartialEq, Eq)]
pub struct RulesFull;

impl GuildAccess {
    /// Настройки из хранилища. Лимит [`MAX_RULES`] здесь не применяется: он ограничивает
    /// добавление правил, а уже сохранённые правила не должны теряться.
    pub fn from_parts(mode: GuildMode, rules: BTreeMap<Subject, Effect>) -> Self {
        Self { mode, rules }
    }

    pub fn rules(&self) -> &BTreeMap<Subject, Effect> {
        &self.rules
    }

    /// Устанавливает правило вместо прежнего и возвращает прежнее.
    pub fn set_rule(
        &mut self,
        subject: Subject,
        effect: Effect,
    ) -> Result<Option<Effect>, RulesFull> {
        if !self.rules.contains_key(&subject) && self.rules.len() >= MAX_RULES {
            return Err(RulesFull);
        }
        Ok(self.rules.insert(subject, effect))
    }

    pub fn remove_rule(&mut self, subject: Subject) -> Option<Effect> {
        self.rules.remove(&subject)
    }

    /// Правила и режим сервера для участника, не управляющего ботом (п. 6–8 в описании модуля).
    fn resolve(&self, user: UserId, roles: &[RoleId]) -> Result<(), Denial> {
        if self.mode == GuildMode::Locked {
            return Err(Denial::Locked);
        }

        let by_user = self.rules.get(&Subject::User(user)).copied();
        let by_roles = || {
            let mut found = None;
            for &role in roles {
                match self.rules.get(&Subject::Role(role)) {
                    // Запрет важнее разрешения: при противоречии доступ закрыт.
                    Some(Effect::Deny) => return Some(Effect::Deny),
                    Some(Effect::Allow) => found = Some(Effect::Allow),
                    None => {}
                }
            }
            found
        };

        match by_user.or_else(by_roles) {
            Some(Effect::Allow) => Ok(()),
            Some(Effect::Deny) => Err(Denial::Denied),
            None if self.mode == GuildMode::Open => Ok(()),
            None => Err(Denial::NotListed),
        }
    }
}

/// Кто обращается к боту.
pub struct Principal<'a> {
    pub user: UserId,
    /// Роли участника (без @everyone — Discord её не перечисляет).
    pub roles: &'a [RoleId],
    pub developer: bool,
    /// Права в канале взаимодействия, вычисленные Discord: у владельца — все, при тайм-ауте —
    /// только просмотр.
    pub permissions: Permissions,
}

/// Причина отказа.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    DeveloperOnly,
    Maintenance,
    Blocked,
    Locked,
    NotListed,
    Denied,
}

impl Denial {
    pub fn message(self) -> String {
        let name = crate::BOT_NAME;
        match self {
            Self::DeveloperOnly => "Команда доступна только разработчикам бота.".into(),
            Self::Maintenance => {
                format!("{name} на обслуживании: сейчас им могут пользоваться только разработчики.")
            }
            Self::Blocked => format!("Разработчики закрыли вам доступ к {name}."),
            Self::Locked => format!("На этом сервере {name} сейчас доступен только администрации."),
            Self::NotListed => format!(
                "На этом сервере {name} доступен по списку, и вас в нём нет. Обратитесь к администрации."
            ),
            Self::Denied => format!("Администрация сервера закрыла вам доступ к {name}."),
        }
    }
}

/// Решение о доступе (порядок проверок — в описании модуля).
pub fn check(
    global: &GlobalAccess,
    guild: &GuildAccess,
    who: &Principal<'_>,
    developer_only: bool,
) -> Result<(), Denial> {
    if who.developer {
        return Ok(());
    }
    if developer_only {
        return Err(Denial::DeveloperOnly);
    }
    if global.mode == GlobalMode::DevOnly {
        return Err(Denial::Maintenance);
    }
    if global.blocked.contains(&who.user) {
        return Err(Denial::Blocked);
    }
    if who.permissions.intersects(MANAGER_PERMISSIONS) {
        return Ok(());
    }
    guild.resolve(who.user, who.roles)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: UserId = UserId::new(10);
    const MEMBER_ROLE: RoleId = RoleId::new(20);
    const MUTED_ROLE: RoleId = RoleId::new(21);

    fn member(roles: &[RoleId]) -> Principal<'_> {
        Principal {
            user: USER,
            roles,
            developer: false,
            permissions: Permissions::SEND_MESSAGES,
        }
    }

    fn guild(mode: GuildMode, rules: &[(Subject, Effect)]) -> GuildAccess {
        let mut access = GuildAccess {
            mode,
            ..Default::default()
        };
        for &(subject, effect) in rules {
            access.set_rule(subject, effect).unwrap();
        }
        access
    }

    fn open() -> GuildAccess {
        GuildAccess::default()
    }

    #[test]
    fn developers_pass_every_level() {
        let global = GlobalAccess {
            mode: GlobalMode::DevOnly,
            blocked: BTreeSet::from([USER]),
        };
        let locked = guild(GuildMode::Locked, &[(Subject::User(USER), Effect::Deny)]);
        let developer = Principal {
            developer: true,
            ..member(&[])
        };
        assert_eq!(check(&global, &locked, &developer, true), Ok(()));
    }

    #[test]
    fn developer_commands_and_maintenance_stop_everyone_else() {
        let owner = Principal {
            permissions: Permissions::all(),
            ..member(&[])
        };
        let global = GlobalAccess::default();
        assert_eq!(
            check(&global, &open(), &owner, true),
            Err(Denial::DeveloperOnly)
        );

        let maintenance = GlobalAccess {
            mode: GlobalMode::DevOnly,
            ..Default::default()
        };
        assert_eq!(
            check(&maintenance, &open(), &owner, false),
            Err(Denial::Maintenance)
        );
    }

    #[test]
    fn global_block_applies_even_to_server_managers() {
        let global = GlobalAccess {
            blocked: BTreeSet::from([USER]),
            ..Default::default()
        };
        let admin = Principal {
            permissions: Permissions::ADMINISTRATOR,
            ..member(&[])
        };
        assert_eq!(check(&global, &open(), &admin, false), Err(Denial::Blocked));
    }

    #[test]
    fn managers_cannot_lock_themselves_out() {
        let global = GlobalAccess::default();
        let hostile = guild(GuildMode::Locked, &[(Subject::User(USER), Effect::Deny)]);
        for permissions in [Permissions::ADMINISTRATOR, Permissions::MANAGE_GUILD] {
            let manager = Principal {
                permissions,
                ..member(&[])
            };
            assert_eq!(check(&global, &hostile, &manager, false), Ok(()));
        }
    }

    #[test]
    fn open_mode_truth_table() {
        let global = GlobalAccess::default();
        let user = |effect| (Subject::User(USER), effect);
        let role = |id, effect| (Subject::Role(id), effect);
        let roles = [MEMBER_ROLE, MUTED_ROLE];

        type Case<'a> = (&'a [(Subject, Effect)], Result<(), Denial>);
        let cases: &[Case<'_>] = &[
            (&[], Ok(())),
            (&[user(Effect::Deny)], Err(Denial::Denied)),
            (&[role(MUTED_ROLE, Effect::Deny)], Err(Denial::Denied)),
            // Среди ролей запрет важнее разрешения.
            (
                &[
                    role(MEMBER_ROLE, Effect::Allow),
                    role(MUTED_ROLE, Effect::Deny),
                ],
                Err(Denial::Denied),
            ),
            // Правило пользователя важнее правил его ролей — в обе стороны.
            (
                &[role(MUTED_ROLE, Effect::Deny), user(Effect::Allow)],
                Ok(()),
            ),
            (
                &[role(MEMBER_ROLE, Effect::Allow), user(Effect::Deny)],
                Err(Denial::Denied),
            ),
            // Правило роли, которой у участника нет, не действует.
            (&[role(RoleId::new(99), Effect::Deny)], Ok(())),
        ];
        for (rules, expected) in cases {
            let access = guild(GuildMode::Open, rules);
            assert_eq!(
                check(&global, &access, &member(&roles), false),
                *expected,
                "{rules:?}"
            );
        }
    }

    #[test]
    fn restricted_mode_requires_a_grant() {
        let global = GlobalAccess::default();
        let roles = [MEMBER_ROLE];

        let nobody = guild(GuildMode::Restricted, &[]);
        assert_eq!(
            check(&global, &nobody, &member(&roles), false),
            Err(Denial::NotListed)
        );

        let by_role = guild(
            GuildMode::Restricted,
            &[(Subject::Role(MEMBER_ROLE), Effect::Allow)],
        );
        assert_eq!(check(&global, &by_role, &member(&roles), false), Ok(()));
        assert_eq!(
            check(&global, &by_role, &member(&[]), false),
            Err(Denial::NotListed)
        );

        let by_user = guild(
            GuildMode::Restricted,
            &[(Subject::User(USER), Effect::Allow)],
        );
        assert_eq!(check(&global, &by_user, &member(&[]), false), Ok(()));
    }

    #[test]
    fn locked_mode_ignores_grants() {
        let global = GlobalAccess::default();
        let locked = guild(GuildMode::Locked, &[(Subject::User(USER), Effect::Allow)]);
        assert_eq!(
            check(&global, &locked, &member(&[]), false),
            Err(Denial::Locked)
        );
    }

    #[test]
    fn rules_are_capped_but_replaceable() {
        let mut access = GuildAccess::default();
        for id in 1..=MAX_RULES as u64 {
            access
                .set_rule(Subject::User(UserId::new(id)), Effect::Allow)
                .unwrap();
        }
        assert_eq!(
            access.set_rule(Subject::User(UserId::new(1_000)), Effect::Allow),
            Err(RulesFull)
        );
        // Замена существующего правила не увеличивает их число.
        assert_eq!(
            access.set_rule(Subject::User(UserId::new(1)), Effect::Deny),
            Ok(Some(Effect::Allow))
        );
    }

    #[test]
    fn keys_round_trip() {
        for mode in GlobalMode::ALL {
            assert_eq!(GlobalMode::from_key(mode.key()), Some(mode));
        }
        for mode in GuildMode::ALL {
            assert_eq!(GuildMode::from_key(mode.key()), Some(mode));
        }
        for effect in [Effect::Allow, Effect::Deny] {
            assert_eq!(Effect::from_key(effect.key()), Some(effect));
        }
        assert_eq!(GuildMode::from_key("closed"), None);
    }
}
