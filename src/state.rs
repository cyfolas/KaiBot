use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serenity::all::{GatewayIntents, ShardId, ShardManager, UserId};

use crate::journal::Journal;
use crate::settings::Settings;

/// Разделяемое состояние процесса.
///
/// Данные о ролях, владельце и правах сервера здесь не хранятся: они читаются из Discord в
/// момент обращения, поэтому не могут разойтись с реальностью. Хранятся только решения
/// администрации и разработчиков, которых в Discord нет, — настройки ([`Settings`]).
pub struct AppState {
    /// Разработчики бота по возрастанию ID — уровень приложения, не зависящий от сервера.
    developers: Box<[UserId]>,
    /// Интенты текущего подключения: от них зависит, какие журналы бот может вести полностью.
    intents: GatewayIntents,
    started_at: Instant,
    /// Устанавливается один раз после сборки клиента (нужен для задержки Gateway).
    shard_manager: OnceLock<Arc<ShardManager>>,
    pub settings: Settings,
    pub journal: Journal,
}

impl AppState {
    pub fn new(
        developers: BTreeSet<UserId>,
        intents: GatewayIntents,
        settings: Settings,
    ) -> Arc<Self> {
        Arc::new(Self {
            developers: developers.into_iter().collect(),
            intents,
            started_at: Instant::now(),
            shard_manager: OnceLock::new(),
            settings,
            journal: Journal::default(),
        })
    }

    pub fn developers(&self) -> &[UserId] {
        &self.developers
    }

    pub fn is_developer(&self, user: UserId) -> bool {
        self.developers.binary_search(&user).is_ok()
    }

    pub fn intents(&self) -> GatewayIntents {
        self.intents
    }

    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn set_shard_manager(&self, manager: Arc<ShardManager>) {
        // Повторная установка невозможна по построению: вызывается один раз при запуске.
        let _ = self.shard_manager.set(manager);
    }

    /// Задержка heartbeat шарда. `None`, пока не получено первое подтверждение (~40 с после подключения).
    pub async fn shard_latency(&self, shard_id: ShardId) -> Option<Duration> {
        let manager = self.shard_manager.get()?;
        let runners = manager.runners.lock().await;
        runners.get(&shard_id).and_then(|runner| runner.latency)
    }
}
