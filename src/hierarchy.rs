//! Иерархия ролей сервера. Чистая логика без обращений к Discord — покрыта тестами.
//!
//! Порядок ролей — как в Discord: выше позиция — старше роль; при равных позициях старше роль с
//! меньшим ID. Высота участника — его высшая роль; владелец сервера выше любой роли.
//!
//! Изменение состава администрации ([`plan`]) разрешено, только если его разрешил бы Discord
//! (пп. 1–2) и оно не делает бота «доверенным посредником» (пп. 3–5):
//!
//! 1. У бота есть «Управлять ролями», и каждая затронутая роль строго ниже высшей роли бота.
//! 2. У вызывающего есть «Управлять ролями», и каждая затронутая роль строго ниже его высшей роли.
//! 3. Участник строго ниже вызывающего: младший не может менять роли старшего или равного.
//!    Исключение — снять роль с самого себя (сложить полномочия).
//! 4. Выдать можно только роль, все права которой есть у вызывающего: иначе он раздал бы права,
//!    которых у него нет.
//! 5. Затрагиваются только административные роли ([`STAFF_PERMISSIONS`]): ни @everyone, ни роли
//!    интеграций (их Discord не даёт выдавать вручную). Боты в администрацию не входят.
//!
//! Владелец сервера не ограничен пп. 2–4.

use std::cmp::Reverse;
use std::collections::HashMap;

use serenity::all::{Guild, GuildId, Mentionable, Permissions, Role, RoleId, UserId};

/// Права, делающие роль административной или модераторской.
pub const STAFF_PERMISSIONS: Permissions = Permissions::ADMINISTRATOR
    .union(Permissions::MANAGE_GUILD)
    .union(Permissions::MANAGE_ROLES)
    .union(Permissions::MANAGE_CHANNELS)
    .union(Permissions::BAN_MEMBERS)
    .union(Permissions::KICK_MEMBERS)
    .union(Permissions::MODERATE_MEMBERS)
    .union(Permissions::MANAGE_MESSAGES);

/// Роль в объёме, нужном для иерархии.
#[derive(Clone, Debug)]
pub struct RoleInfo {
    pub id: RoleId,
    pub position: u16,
    pub permissions: Permissions,
    /// Роль интеграции, бустеров или подписки: Discord не даёт выдавать её вручную.
    pub managed: bool,
}

impl From<&Role> for RoleInfo {
    fn from(role: &Role) -> Self {
        Self {
            id: role.id,
            position: role.position,
            permissions: role.permissions,
            managed: role.managed,
        }
    }
}

impl RoleInfo {
    pub fn rank(&self) -> Rank {
        Rank {
            position: self.position,
            id: Reverse(self.id),
        }
    }
}

/// Место роли в иерархии: больше — старше.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank {
    position: u16,
    id: Reverse<RoleId>,
}

/// Высота участника в иерархии. Порядок вариантов — порядок высоты.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Height {
    /// Нет ролей, кроме @everyone: ниже любой роли.
    Base,
    /// Высшая роль участника.
    Role(Rank),
    /// Владелец сервера: выше любой роли.
    Owner,
}

impl Height {
    /// Строго ли роль ниже этой высоты.
    pub fn above(self, role: &RoleInfo) -> bool {
        Height::Role(role.rank()) < self
    }
}

/// Участник в объёме, нужном для иерархии.
#[derive(Clone, Copy)]
pub struct Person<'a> {
    pub id: UserId,
    pub roles: &'a [RoleId],
    pub bot: bool,
}

/// Роли сервера и его владелец.
pub struct Hierarchy {
    guild_id: GuildId,
    owner: UserId,
    roles: HashMap<RoleId, RoleInfo>,
}

impl Hierarchy {
    pub fn new(
        guild_id: GuildId,
        owner: UserId,
        roles: impl IntoIterator<Item = RoleInfo>,
    ) -> Self {
        Self {
            guild_id,
            owner,
            roles: roles.into_iter().map(|role| (role.id, role)).collect(),
        }
    }

    pub fn of(guild: &Guild) -> Self {
        Self::new(
            guild.id,
            guild.owner_id,
            guild.roles.values().map(RoleInfo::from),
        )
    }

    pub fn role(&self, id: RoleId) -> Option<&RoleInfo> {
        self.roles.get(&id)
    }

    /// Роль @everyone: её ID совпадает с ID сервера.
    pub fn is_everyone(&self, id: RoleId) -> bool {
        id.get() == self.guild_id.get()
    }

    /// Административная роль: с правами управления, не @everyone и не роль интеграции.
    pub fn is_staff(&self, role: &RoleInfo) -> bool {
        !self.is_everyone(role.id)
            && !role.managed
            && role.permissions.intersects(STAFF_PERMISSIONS)
    }

    /// Роли, кроме @everyone, от старшей к младшей.
    pub fn sorted(&self) -> Vec<&RoleInfo> {
        let mut roles: Vec<&RoleInfo> = self
            .roles
            .values()
            .filter(|role| !self.is_everyone(role.id))
            .collect();
        roles.sort_by_key(|role| Reverse(role.rank()));
        roles
    }

    /// Административные роли от старшей к младшей.
    pub fn staff(&self) -> Vec<&RoleInfo> {
        let mut roles = self.sorted();
        roles.retain(|role| self.is_staff(role));
        roles
    }

    pub fn height(&self, person: Person<'_>) -> Height {
        if person.id == self.owner {
            return Height::Owner;
        }
        person
            .roles
            .iter()
            .filter_map(|id| self.roles.get(id))
            .map(RoleInfo::rank)
            .max()
            .map_or(Height::Base, Height::Role)
    }

    /// Права на уровне сервера: @everyone и роли участника; у администратора и владельца — все.
    pub fn permissions(&self, person: Person<'_>) -> Permissions {
        if person.id == self.owner {
            return Permissions::all();
        }
        let everyone = RoleId::new(self.guild_id.get());
        let granted = std::iter::once(&everyone)
            .chain(person.roles)
            .filter_map(|id| self.roles.get(id))
            .fold(Permissions::empty(), |acc, role| acc | role.permissions);
        if granted.administrator() {
            Permissions::all()
        } else {
            granted
        }
    }

    /// Положение бота: его высшая роль и административные роли, которыми он может управлять.
    pub fn standing(&self, bot: Person<'_>) -> Standing {
        let sorted = self.sorted();
        let height = self.height(bot);
        let top = sorted
            .iter()
            .position(|role| Height::Role(role.rank()) == height)
            .map(|index| (sorted[index].id, index + 1));
        let (manageable, unmanageable) = self
            .staff()
            .into_iter()
            .map(|role| role.id)
            .partition(|&id| self.roles.get(&id).is_some_and(|role| height.above(role)));

        Standing {
            top,
            total: sorted.len(),
            manage_roles: self.permissions(bot).manage_roles(),
            manageable,
            unmanageable,
        }
    }
}

/// Положение бота в иерархии сервера.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Standing {
    /// Высшая роль бота и её место сверху (1 — самая старшая) среди [`Self::total`] ролей.
    pub top: Option<(RoleId, usize)>,
    /// Число ролей сервера, кроме @everyone.
    pub total: usize,
    /// Есть ли у бота «Управлять ролями» (или «Администратор»).
    pub manage_roles: bool,
    /// Административные роли ниже роли бота — ими он может управлять.
    pub manageable: Vec<RoleId>,
    /// Административные роли не ниже роли бота — ими он управлять не может.
    pub unmanageable: Vec<RoleId>,
}

impl Standing {
    /// Те же ли возможности у бота: высшая роль, право управлять ролями, недоступные роли.
    /// Место роли не сравнивается: оно сдвигается от создания любых ролей выше.
    pub fn same_reach(&self, other: &Self) -> bool {
        self.top.map(|(role, _)| role) == other.top.map(|(role, _)| role)
            && self.manage_roles == other.manage_roles
            && self.unmanageable == other.unmanageable
    }

    /// Описание для людей: позиция роли и чем бот может управлять.
    pub fn describe(&self) -> String {
        let mut text = match self.top {
            Some((role, place)) => format!(
                "Роль бота: {} — {place}-я сверху из {}.",
                role.mention(),
                self.total
            ),
            None => "У бота нет ролей, кроме @everyone.".to_string(),
        };
        if !self.manage_roles {
            text += "\n⚠️ У бота нет права «Управлять ролями»: менять состав администрации он не может.";
        } else if !self.unmanageable.is_empty() {
            text += &format!(
                "\n⚠️ Роли не ниже роли бота — ими он управлять не может: {}. \
                 Чтобы это исправить, поднимите роль бота выше в настройках сервера.",
                mentions(&self.unmanageable)
            );
        } else {
            text += "\n✅ Бот может управлять всеми административными ролями.";
        }
        text
    }
}

/// Запрошенное изменение состава администрации.
#[derive(Clone, Copy, Debug)]
pub enum Request {
    /// Выдать административную роль.
    Add(RoleId),
    /// Снять роль; без роли — все административные роли участника.
    Remove(Option<RoleId>),
    /// Назначить на должность: выдать роль и снять остальные административные роли.
    Set(RoleId),
}

/// Что сделать с ролями участника.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub add: Option<RoleId>,
    pub remove: Vec<RoleId>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.add.is_none() && self.remove.is_empty()
    }
}

/// Почему изменение невозможно.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    UnknownRole(RoleId),
    Everyone,
    Managed(RoleId),
    NotStaff(RoleId),
    TargetIsBot,
    TargetIsOwner,
    TargetNotLower,
    ActorCannotManageRoles,
    RoleNotBelowActor(RoleId),
    Escalation { role: RoleId, missing: Permissions },
    BotCannotManageRoles,
    RoleNotBelowBot(RoleId),
    AlreadyHas(RoleId),
    DoesNotHave(RoleId),
    NotInStaff,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Self::UnknownRole(role) => format!("Роль {} не найдена на сервере.", role.mention()),
            Self::Everyone => {
                "@everyone есть у всех участников — её нельзя выдать или снять.".into()
            }
            Self::Managed(role) => format!(
                "{} управляется интеграцией (бот, бусты, подписки) — Discord не даёт выдавать её вручную.",
                role.mention()
            ),
            Self::NotStaff(role) => format!(
                "{} не административная роль: у неё нет прав управления сервером или модерации.",
                role.mention()
            ),
            Self::TargetIsBot => "Боты не входят в администрацию.".into(),
            Self::TargetIsOwner => "Роли владельца сервера может менять только он сам.".into(),
            Self::TargetNotLower => {
                "Участник не ниже вас по иерархии: менять роли старших и равных нельзя.".into()
            }
            Self::ActorCannotManageRoles => "У вас нет права «Управлять ролями» на сервере.".into(),
            Self::RoleNotBelowActor(role) => format!(
                "{} не ниже вашей высшей роли — управлять ею вы не можете.",
                role.mention()
            ),
            Self::Escalation { role, missing } => format!(
                "У {} есть права, которых нет у вас: {}. Выдать её может тот, у кого они есть.",
                role.mention(),
                crate::text::describe_permissions(*missing)
            ),
            Self::BotCannotManageRoles => {
                "У бота нет права «Управлять ролями». Выдайте его роли бота в настройках сервера."
                    .into()
            }
            Self::RoleNotBelowBot(role) => format!(
                "{} не ниже роли бота — Discord не даст ему ею управлять. \
                 Поднимите роль бота выше в настройках сервера.",
                role.mention()
            ),
            Self::AlreadyHas(role) => format!("У участника уже есть {}.", role.mention()),
            Self::DoesNotHave(role) => format!("У участника нет {}.", role.mention()),
            Self::NotInStaff => "Участник не входит в администрацию.".into(),
        }
    }
}

/// План изменения ролей `target` по запросу `actor`, выполняемого ботом `bot`.
/// Проверяются все правила из описания модуля; при любом нарушении не выполняется ничего.
pub fn plan(
    hierarchy: &Hierarchy,
    actor: Person<'_>,
    bot: Person<'_>,
    target: Person<'_>,
    request: Request,
) -> Result<Plan, Refusal> {
    if target.bot {
        return Err(Refusal::TargetIsBot);
    }

    let actor_height = hierarchy.height(actor);
    let owner = actor_height == Height::Owner;
    let self_removal = actor.id == target.id && matches!(request, Request::Remove(_));
    if !owner {
        if target.id == hierarchy.owner {
            return Err(Refusal::TargetIsOwner);
        }
        if !self_removal && hierarchy.height(target) >= actor_height {
            return Err(Refusal::TargetNotLower);
        }
    }

    let has = |role: RoleId| target.roles.contains(&role);
    let staff_of_target: Vec<RoleId> = hierarchy
        .staff()
        .into_iter()
        .map(|role| role.id)
        .filter(|&role| has(role))
        .collect();

    let plan = match request {
        Request::Add(role) => {
            staff_role(hierarchy, role)?;
            if has(role) {
                return Err(Refusal::AlreadyHas(role));
            }
            Plan {
                add: Some(role),
                remove: Vec::new(),
            }
        }
        Request::Remove(Some(role)) => {
            staff_role(hierarchy, role)?;
            if !has(role) {
                return Err(Refusal::DoesNotHave(role));
            }
            Plan {
                add: None,
                remove: vec![role],
            }
        }
        Request::Remove(None) => {
            if staff_of_target.is_empty() {
                return Err(Refusal::NotInStaff);
            }
            Plan {
                add: None,
                remove: staff_of_target,
            }
        }
        Request::Set(role) => {
            staff_role(hierarchy, role)?;
            let remove: Vec<RoleId> = staff_of_target
                .into_iter()
                .filter(|&other| other != role)
                .collect();
            if has(role) && remove.is_empty() {
                return Err(Refusal::AlreadyHas(role));
            }
            Plan {
                add: (!has(role)).then_some(role),
                remove,
            }
        }
    };

    let actor_permissions = hierarchy.permissions(actor);
    if !actor_permissions.manage_roles() {
        return Err(Refusal::ActorCannotManageRoles);
    }
    if !hierarchy.permissions(bot).manage_roles() {
        return Err(Refusal::BotCannotManageRoles);
    }
    let bot_height = hierarchy.height(bot);

    for id in plan.add.iter().chain(&plan.remove) {
        let role = hierarchy.role(*id).ok_or(Refusal::UnknownRole(*id))?;
        if !actor_height.above(role) {
            return Err(Refusal::RoleNotBelowActor(*id));
        }
        if !bot_height.above(role) {
            return Err(Refusal::RoleNotBelowBot(*id));
        }
    }
    if let Some(id) = plan.add {
        let missing = hierarchy.role(id).map_or(Permissions::empty(), |role| {
            role.permissions - actor_permissions
        });
        if !missing.is_empty() {
            return Err(Refusal::Escalation { role: id, missing });
        }
    }
    Ok(plan)
}

/// Проверяет, что роль существует и административная (п. 5).
fn staff_role(hierarchy: &Hierarchy, id: RoleId) -> Result<&RoleInfo, Refusal> {
    if hierarchy.is_everyone(id) {
        return Err(Refusal::Everyone);
    }
    let role = hierarchy.role(id).ok_or(Refusal::UnknownRole(id))?;
    if role.managed {
        return Err(Refusal::Managed(id));
    }
    if !hierarchy.is_staff(role) {
        return Err(Refusal::NotStaff(id));
    }
    Ok(role)
}

pub fn mentions(roles: &[RoleId]) -> String {
    roles
        .iter()
        .map(|role| role.mention().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUILD: GuildId = GuildId::new(1);
    const OWNER: UserId = UserId::new(100);
    const ADMIN: UserId = UserId::new(101);
    const MODERATOR: UserId = UserId::new(102);
    const MEMBER: UserId = UserId::new(103);
    const BOT: UserId = UserId::new(200);

    // Роли сверху вниз: admin (5), bot (4), moderator (3), helper (2), member (1), @everyone (0).
    const ADMIN_ROLE: RoleId = RoleId::new(10);
    const BOT_ROLE: RoleId = RoleId::new(11);
    const MOD_ROLE: RoleId = RoleId::new(12);
    const HELPER_ROLE: RoleId = RoleId::new(13);
    const MEMBER_ROLE: RoleId = RoleId::new(14);

    fn role(id: RoleId, position: u16, permissions: Permissions, managed: bool) -> RoleInfo {
        RoleInfo {
            id,
            position,
            permissions,
            managed,
        }
    }

    fn hierarchy() -> Hierarchy {
        Hierarchy::new(
            GUILD,
            OWNER,
            [
                role(
                    RoleId::new(GUILD.get()),
                    0,
                    Permissions::SEND_MESSAGES,
                    false,
                ),
                role(ADMIN_ROLE, 5, Permissions::ADMINISTRATOR, false),
                role(
                    BOT_ROLE,
                    4,
                    Permissions::MANAGE_ROLES | Permissions::BAN_MEMBERS,
                    true,
                ),
                role(
                    MOD_ROLE,
                    3,
                    Permissions::MANAGE_ROLES
                        | Permissions::KICK_MEMBERS
                        | Permissions::BAN_MEMBERS,
                    false,
                ),
                role(HELPER_ROLE, 2, Permissions::MANAGE_MESSAGES, false),
                role(MEMBER_ROLE, 1, Permissions::empty(), false),
            ],
        )
    }

    fn person(id: UserId, roles: &[RoleId]) -> Person<'_> {
        Person {
            id,
            roles,
            bot: false,
        }
    }

    fn bot() -> Person<'static> {
        Person {
            id: BOT,
            roles: &[BOT_ROLE],
            bot: true,
        }
    }

    #[test]
    fn ranks_follow_discord_order() {
        let low_id = role(RoleId::new(1), 3, Permissions::empty(), false);
        let high_id = role(RoleId::new(2), 3, Permissions::empty(), false);
        let higher = role(RoleId::new(3), 4, Permissions::empty(), false);
        // При равных позициях старше роль с меньшим ID.
        assert!(low_id.rank() > high_id.rank());
        assert!(higher.rank() > low_id.rank());
        assert!(Height::Owner > Height::Role(higher.rank()));
        assert!(Height::Role(high_id.rank()) > Height::Base);
    }

    #[test]
    fn heights_and_permissions() {
        let h = hierarchy();
        assert_eq!(h.height(person(OWNER, &[])), Height::Owner);
        assert_eq!(h.height(person(MEMBER, &[])), Height::Base);
        assert_eq!(
            h.height(person(MODERATOR, &[HELPER_ROLE, MOD_ROLE])),
            Height::Role(h.role(MOD_ROLE).unwrap().rank())
        );
        assert_eq!(
            h.permissions(person(ADMIN, &[ADMIN_ROLE])),
            Permissions::all()
        );
        assert_eq!(h.permissions(person(OWNER, &[])), Permissions::all());
        assert_eq!(
            h.permissions(person(MEMBER, &[MEMBER_ROLE])),
            Permissions::SEND_MESSAGES
        );
    }

    #[test]
    fn staff_excludes_everyone_managed_and_powerless_roles() {
        let h = hierarchy();
        let staff: Vec<RoleId> = h.staff().iter().map(|role| role.id).collect();
        assert_eq!(staff, vec![ADMIN_ROLE, MOD_ROLE, HELPER_ROLE]);
    }

    #[test]
    fn bot_standing_reports_position_and_reach() {
        let standing = hierarchy().standing(bot());
        assert_eq!(standing.top, Some((BOT_ROLE, 2)));
        assert_eq!(standing.total, 5);
        assert!(standing.manage_roles);
        assert_eq!(standing.manageable, vec![MOD_ROLE, HELPER_ROLE]);
        assert_eq!(standing.unmanageable, vec![ADMIN_ROLE]);
        assert!(standing.describe().contains("2-я сверху из 5"));
    }

    #[test]
    fn moderator_promotes_member_to_helper() {
        let h = hierarchy();
        let moderator = person(MODERATOR, &[MOD_ROLE]);
        let member = person(MEMBER, &[MEMBER_ROLE]);
        // У модератора нет «Управлять сообщениями», которое даёт роль помощника.
        assert_eq!(
            plan(&h, moderator, bot(), member, Request::Add(HELPER_ROLE)),
            Err(Refusal::Escalation {
                role: HELPER_ROLE,
                missing: Permissions::MANAGE_MESSAGES
            })
        );

        let admin = person(ADMIN, &[ADMIN_ROLE]);
        assert_eq!(
            plan(&h, admin, bot(), member, Request::Add(HELPER_ROLE)),
            Ok(Plan {
                add: Some(HELPER_ROLE),
                remove: vec![]
            })
        );
    }

    #[test]
    fn discord_hierarchy_limits_actor_and_bot() {
        let h = hierarchy();
        let moderator = person(MODERATOR, &[MOD_ROLE]);
        let member = person(MEMBER, &[]);
        // Своя высшая роль — не ниже себя.
        assert_eq!(
            plan(&h, moderator, bot(), member, Request::Add(MOD_ROLE)),
            Err(Refusal::RoleNotBelowActor(MOD_ROLE))
        );
        // Владелец может всё, что может бот, но не выше роли бота.
        let owner = person(OWNER, &[]);
        assert_eq!(
            plan(&h, owner, bot(), member, Request::Add(ADMIN_ROLE)),
            Err(Refusal::RoleNotBelowBot(ADMIN_ROLE))
        );
        let powerless_bot = Person {
            roles: &[],
            ..bot()
        };
        assert_eq!(
            plan(&h, owner, powerless_bot, member, Request::Add(MOD_ROLE)),
            Err(Refusal::BotCannotManageRoles)
        );
    }

    #[test]
    fn targets_must_be_strictly_lower() {
        let h = hierarchy();
        let moderator = person(MODERATOR, &[MOD_ROLE]);
        let peer = person(UserId::new(150), &[MOD_ROLE]);
        assert_eq!(
            plan(&h, moderator, bot(), peer, Request::Remove(Some(MOD_ROLE))),
            Err(Refusal::TargetNotLower)
        );
        assert_eq!(
            plan(
                &h,
                moderator,
                bot(),
                person(OWNER, &[]),
                Request::Remove(None)
            ),
            Err(Refusal::TargetIsOwner)
        );
        let robot = Person {
            bot: true,
            ..person(UserId::new(151), &[])
        };
        assert_eq!(
            plan(&h, person(OWNER, &[]), bot(), robot, Request::Add(MOD_ROLE)),
            Err(Refusal::TargetIsBot)
        );
    }

    #[test]
    fn stepping_down_is_allowed_but_self_promotion_is_not() {
        let h = hierarchy();
        let both = [MOD_ROLE, HELPER_ROLE];
        let moderator = person(MODERATOR, &both);
        assert_eq!(
            plan(
                &h,
                moderator,
                bot(),
                moderator,
                Request::Remove(Some(HELPER_ROLE))
            ),
            Ok(Plan {
                add: None,
                remove: vec![HELPER_ROLE]
            })
        );
        // Высшую роль снять нельзя: она не ниже самого себя.
        assert_eq!(
            plan(&h, moderator, bot(), moderator, Request::Remove(None)),
            Err(Refusal::RoleNotBelowActor(MOD_ROLE))
        );
        let helper = person(MODERATOR, &[MOD_ROLE]);
        assert_eq!(
            plan(&h, helper, bot(), helper, Request::Add(HELPER_ROLE)),
            Err(Refusal::TargetNotLower)
        );
    }

    #[test]
    fn set_replaces_other_staff_roles() {
        let h = hierarchy();
        let owner = person(OWNER, &[]);
        let helper = person(MEMBER, &[HELPER_ROLE, MEMBER_ROLE]);
        assert_eq!(
            plan(&h, owner, bot(), helper, Request::Set(MOD_ROLE)),
            Ok(Plan {
                add: Some(MOD_ROLE),
                remove: vec![HELPER_ROLE]
            })
        );
        assert_eq!(
            plan(&h, owner, bot(), helper, Request::Set(HELPER_ROLE)),
            Err(Refusal::AlreadyHas(HELPER_ROLE))
        );
        assert_eq!(
            plan(&h, owner, bot(), helper, Request::Remove(None)),
            Ok(Plan {
                add: None,
                remove: vec![HELPER_ROLE]
            })
        );
    }

    #[test]
    fn only_staff_roles_are_touched() {
        let h = hierarchy();
        let owner = person(OWNER, &[]);
        let member = person(MEMBER, &[MEMBER_ROLE]);
        assert_eq!(
            plan(&h, owner, bot(), member, Request::Add(MEMBER_ROLE)),
            Err(Refusal::NotStaff(MEMBER_ROLE))
        );
        assert_eq!(
            plan(&h, owner, bot(), member, Request::Add(BOT_ROLE)),
            Err(Refusal::Managed(BOT_ROLE))
        );
        assert_eq!(
            plan(
                &h,
                owner,
                bot(),
                member,
                Request::Add(RoleId::new(GUILD.get()))
            ),
            Err(Refusal::Everyone)
        );
        assert_eq!(
            plan(&h, owner, bot(), member, Request::Remove(None)),
            Err(Refusal::NotInStaff)
        );
    }
}
