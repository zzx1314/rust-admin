//! 内存版令牌存储（`APP_TOKEN_STORE=memory`）测试。
//!
//! 它替代的是 Redis 的会话存储，所以要保证语义一致：
//!   1. access / refresh 两组令牌互不干扰，同一用户重复登录覆盖旧值；
//!   2. 登出（delete）后取不到；
//!   3. TTL 到点后自动视为不存在（等价 Redis 的键过期）；
//!   4. 克隆出来的句柄共享同一份数据（`AppState` 里到处克隆的是同一个 store）。

use std::time::Duration;

use x_rust::common::traits::TokenStore;
use x_rust::system::auth::memory_store::InMemoryTokenStore;

const TTL: u64 = 3600;

#[tokio::test]
async fn stores_and_returns_access_token() {
    let store = InMemoryTokenStore::new();

    store.set_token("1", "access-a", TTL).await.unwrap();
    assert_eq!(
        store.get_token("1").await.unwrap().as_deref(),
        Some("access-a")
    );

    // 再次登录覆盖旧值，不会残留两份。
    store.set_token("1", "access-b", TTL).await.unwrap();
    assert_eq!(
        store.get_token("1").await.unwrap().as_deref(),
        Some("access-b")
    );

    // 多用户互不影响。
    store.set_token("2", "access-c", TTL).await.unwrap();
    assert_eq!(
        store.get_token("2").await.unwrap().as_deref(),
        Some("access-c")
    );
    assert_eq!(
        store.get_token("1").await.unwrap().as_deref(),
        Some("access-b")
    );
}

#[tokio::test]
async fn access_and_refresh_tokens_do_not_collide() {
    let store = InMemoryTokenStore::new();

    store.set_token("1", "access", TTL).await.unwrap();
    store.set_refresh_token("1", "refresh", TTL).await.unwrap();

    assert_eq!(
        store.get_token("1").await.unwrap().as_deref(),
        Some("access")
    );
    assert_eq!(
        store.get_refresh_token("1").await.unwrap().as_deref(),
        Some("refresh")
    );

    // 只删 access 不影响 refresh（与 Redis 版键名隔离行为一致）。
    store.delete_token("1").await.unwrap();
    assert!(store.get_token("1").await.unwrap().is_none());
    assert_eq!(
        store.get_refresh_token("1").await.unwrap().as_deref(),
        Some("refresh")
    );

    store.delete_refresh_token("1").await.unwrap();
    assert!(store.get_refresh_token("1").await.unwrap().is_none());
}

#[tokio::test]
async fn expired_token_is_treated_as_missing() {
    let store = InMemoryTokenStore::new();

    // ttl = 0 → 到期时间就是现在，读出来即已过期。
    store.set_token("1", "short-lived", 0).await.unwrap();
    assert!(
        store.get_token("1").await.unwrap().is_none(),
        "TTL 到点后必须当作不存在"
    );
}

#[tokio::test]
async fn cloned_handles_share_the_same_storage() {
    let store = InMemoryTokenStore::new();
    let clone = store.clone();

    store.set_token("1", "shared", TTL).await.unwrap();
    assert_eq!(
        clone.get_token("1").await.unwrap().as_deref(),
        Some("shared")
    );

    clone.delete_token("1").await.unwrap();
    assert!(store.get_token("1").await.unwrap().is_none());
}

#[tokio::test]
async fn long_ttl_survives_a_short_wait() {
    let store = InMemoryTokenStore::new();
    store.set_token("1", "valid", 60).await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        store.get_token("1").await.unwrap().as_deref(),
        Some("valid")
    );
}

#[tokio::test]
async fn writes_purge_expired_entries() {
    // 长期运行的服务不能因为过期键堆积而漏内存：写入时顺手清理。
    let store = InMemoryTokenStore::new();
    for user in 0..200 {
        store
            .set_token(&user.to_string(), "expired", 0)
            .await
            .unwrap();
    }

    store.set_token("keep", "alive", TTL).await.unwrap();

    assert_eq!(
        store.get_token("keep").await.unwrap().as_deref(),
        Some("alive")
    );
    for user in 0..200 {
        assert!(store.get_token(&user.to_string()).await.unwrap().is_none());
    }
}
