-- Настройки Axiom.
--
-- ID Discord (snowflake) хранятся как INTEGER с тем же битовым представлением (u64 ↔ i64 без
-- потерь): SQLite знает только знаковые 64-битные числа. Ноль не бывает ID — это проверяет CHECK.
-- Перечисления — TEXT с CHECK: значения читаемы в базе и не зависят от порядка вариантов в коде.

-- Уровень приложения, управляют разработчики. Ровно одна строка.
CREATE TABLE global_settings (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    mode TEXT    NOT NULL CHECK (mode IN ('public', 'dev-only'))
) STRICT;

INSERT INTO global_settings (id, mode) VALUES (1, 'public');

-- Пользователи, которым разработчики закрыли доступ к боту на всех серверах.
CREATE TABLE blocked_users (
    user_id INTEGER PRIMARY KEY CHECK (user_id <> 0)
) STRICT;

-- Уровень сервера, управляет администрация сервера. Нет строки — настройки по умолчанию.
CREATE TABLE guild_settings (
    guild_id    INTEGER PRIMARY KEY CHECK (guild_id <> 0),
    access_mode TEXT    NOT NULL CHECK (access_mode IN ('open', 'restricted', 'locked')),
    message_log INTEGER CHECK (message_log <> 0),
    voice_log   INTEGER CHECK (voice_log <> 0),
    system_log  INTEGER CHECK (system_log <> 0)
) STRICT;

-- Правила доступа: не больше одного правила на пользователя или роль.
CREATE TABLE access_rules (
    guild_id     INTEGER NOT NULL REFERENCES guild_settings (guild_id) ON DELETE CASCADE,
    subject_kind TEXT    NOT NULL CHECK (subject_kind IN ('user', 'role')),
    subject_id   INTEGER NOT NULL CHECK (subject_id <> 0),
    effect       TEXT    NOT NULL CHECK (effect IN ('allow', 'deny')),
    PRIMARY KEY (guild_id, subject_kind, subject_id)
) STRICT, WITHOUT ROWID;
