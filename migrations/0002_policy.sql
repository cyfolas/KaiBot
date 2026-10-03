-- Политика прав сервера (см. domain::policy). Нет строки — политика не настроена.
--
-- Наборы прав хранятся как INTEGER: биты Discord (u64) в том же битовом представлении.

CREATE TABLE guild_policy (
    guild_id             INTEGER PRIMARY KEY REFERENCES guild_settings (guild_id) ON DELETE CASCADE,
    member_role          INTEGER NOT NULL CHECK (member_role <> 0),
    unverified_role      INTEGER CHECK (unverified_role <> 0),
    chat_mute_role       INTEGER CHECK (chat_mute_role <> 0),
    voice_mute_role      INTEGER CHECK (voice_mute_role <> 0),
    everyone_permissions INTEGER NOT NULL,
    member_permissions   INTEGER NOT NULL
) STRICT;

-- Назначение каналов, заданное явно; остальные каналы классифицируются по их оверрайтам.
CREATE TABLE channel_policy (
    guild_id   INTEGER NOT NULL REFERENCES guild_policy (guild_id) ON DELETE CASCADE,
    channel_id INTEGER NOT NULL CHECK (channel_id <> 0),
    class      TEXT    NOT NULL
        CHECK (class IN ('public', 'private', 'read-only', 'verification', 'ignored')),
    PRIMARY KEY (guild_id, channel_id)
) STRICT, WITHOUT ROWID;

-- Роли класса: список доступа закрытого канала или авторы канала только для чтения.
CREATE TABLE channel_policy_roles (
    guild_id   INTEGER NOT NULL,
    channel_id INTEGER NOT NULL,
    role_id    INTEGER NOT NULL CHECK (role_id <> 0),
    PRIMARY KEY (guild_id, channel_id, role_id),
    FOREIGN KEY (guild_id, channel_id)
        REFERENCES channel_policy (guild_id, channel_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;
