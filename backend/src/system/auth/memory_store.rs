//! 进程内令牌存储：不需要 Redis 的会话后端。
//!
//! 语义与 [`super::repository::RedisTokenStore`] 对齐（access / refresh 两组键、
//! 带 TTL、取到过期键即当作不存在），区别只有存储位置：
//!
//! | | Redis | 内存（本模块） |
//! | --- | --- | --- |
//! | 依赖 | 需要 redis-server | 无 |
//! | 进程重启 | 会话保留 | **所有人重新登录** |
//! | 多实例 | 共享会话 | 各自独立 |
//!
//! 因此它适合「单机部署、装不了也不想装 Redis」的场景，由
//! `APP_TOKEN_STORE=memory`（或 `token_store = "memory"`）开启。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::common::error::AppError;
use crate::common::traits::{DynFuture, TokenStore};

/// 一条令牌记录。`expires_at` 为 `None` 表示永不过期。
#[derive(Debug, Clone)]
struct Entry {
    value: String,
    expires_at: Option<Instant>,
}

impl Entry {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<String, Entry>,
}

impl Inner {
    /// 写一条记录，顺手清掉已经过期的键，避免长期运行后内存只增不减。
    fn insert(&mut self, key: String, value: String, ttl_secs: u64) {
        let now = Instant::now();
        self.entries.retain(|_, entry| !entry.is_expired(now));
        self.entries.insert(
            key,
            Entry {
                value,
                expires_at: now.checked_add(Duration::from_secs(ttl_secs)),
            },
        );
    }

    fn get(&mut self, key: &str) -> Option<String> {
        let now = Instant::now();
        match self.entries.get(key) {
            Some(entry) if entry.is_expired(now) => {
                self.entries.remove(key);
                None
            }
            Some(entry) => Some(entry.value.clone()),
            None => None,
        }
    }

    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

/// 内存版 [`TokenStore`]。克隆出来的句柄共享同一份数据。
#[derive(Debug, Clone, Default)]
pub struct InMemoryTokenStore {
    inner: Arc<Mutex<Inner>>,
}

impl InMemoryTokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<T>(&self, action: impl FnOnce(&mut Inner) -> T) -> T {
        // 锁 poisoning 只可能来自 panic 期间持锁；恢复内层数据比直接崩掉服务好。
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        action(&mut guard)
    }

    fn set(&self, key: String, token: String, ttl_secs: u64) -> Result<(), AppError> {
        self.with(|inner| inner.insert(key, token, ttl_secs));
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>, AppError> {
        Ok(self.with(|inner| inner.get(key)))
    }

    fn delete(&self, key: &str) -> Result<(), AppError> {
        self.with(|inner| inner.remove(key));
        Ok(())
    }
}

impl TokenStore for InMemoryTokenStore {
    fn set_token(
        &self,
        user_id: &str,
        token: &str,
        ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let (store, key, token) = (self.clone(), access_key(user_id), token.to_string());
        Box::pin(async move { store.set(key, token, ttl_secs) })
    }

    fn get_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let (store, key) = (self.clone(), access_key(user_id));
        Box::pin(async move { store.get(&key) })
    }

    fn delete_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let (store, key) = (self.clone(), access_key(user_id));
        Box::pin(async move { store.delete(&key) })
    }

    fn set_refresh_token(
        &self,
        user_id: &str,
        token: &str,
        ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let (store, key, token) = (self.clone(), refresh_key(user_id), token.to_string());
        Box::pin(async move { store.set(key, token, ttl_secs) })
    }

    fn get_refresh_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let (store, key) = (self.clone(), refresh_key(user_id));
        Box::pin(async move { store.get(&key) })
    }

    fn delete_refresh_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let (store, key) = (self.clone(), refresh_key(user_id));
        Box::pin(async move { store.delete(&key) })
    }
}

/// 与 Redis 版保持完全一致的键名，便于两种实现互相切换。
fn access_key(user_id: &str) -> String {
    format!("auth:token:{}", user_id)
}

fn refresh_key(user_id: &str) -> String {
    format!("auth:refresh:{}", user_id)
}
