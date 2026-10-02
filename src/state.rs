use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serenity::all::{ShardId, ShardManager, UserId};

/// Разделяемое состояние процесса.
///
/// Намеренно не содержит данных конкретного сервера: роли, владелец и права читаются из Discord
/// в момент обращения, поэтому состояние не может разойтись с реальностью.
pub struct AppState {
    /// Разработчики бота по возрастанию ID — уровень приложения, не зависящий от сервера.
    developers: Box<[UserId]>,
    started_at: Instant,
    /// Устанавливается один раз после сборки клиента (нужен для задержки Gateway).
    shard_manager: OnceLock<Arc<ShardManager>>,
}

impl AppState {
    pub fn new(developers: BTreeSet<UserId>) -> Arc<Self> {
        Arc::new(Self {
            developers: developers.into_iter().collect(),
            started_at: Instant::now(),
            shard_manager: OnceLock::new(),
        })
    }

    pub fn developers(&self) -> &[UserId] {
        &self.developers
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
