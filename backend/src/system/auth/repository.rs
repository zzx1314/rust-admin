use redis::Client;

use crate::common::error::AppError;
use crate::common::traits::{DynFuture, TokenStore};

pub struct RedisTokenStore {
    redis_client: Client,
}

impl RedisTokenStore {
    pub fn new(redis_url: &str) -> Self {
        let redis_client = Client::open(redis_url).expect("Invalid Redis URL");
        Self { redis_client }
    }
}

impl TokenStore for RedisTokenStore {
    fn set_token(
        &self,
        user_id: &str,
        token: &str,
        ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let redis_client = self.redis_client.clone();
        let redis_key = access_key(user_id);
        let token = token.to_string();
        Box::pin(async move { set_with_ttl(&redis_client, &redis_key, &token, ttl_secs).await })
    }

    fn get_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let redis_client = self.redis_client.clone();
        let redis_key = access_key(user_id);
        Box::pin(async move { get_key(&redis_client, &redis_key).await })
    }

    fn delete_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let redis_client = self.redis_client.clone();
        let redis_key = access_key(user_id);
        Box::pin(async move { del_key(&redis_client, &redis_key).await })
    }

    fn set_refresh_token(
        &self,
        user_id: &str,
        token: &str,
        ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let redis_client = self.redis_client.clone();
        let redis_key = refresh_key(user_id);
        let token = token.to_string();
        Box::pin(async move { set_with_ttl(&redis_client, &redis_key, &token, ttl_secs).await })
    }

    fn get_refresh_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let redis_client = self.redis_client.clone();
        let redis_key = refresh_key(user_id);
        Box::pin(async move { get_key(&redis_client, &redis_key).await })
    }

    fn delete_refresh_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let redis_client = self.redis_client.clone();
        let redis_key = refresh_key(user_id);
        Box::pin(async move { del_key(&redis_client, &redis_key).await })
    }
}

fn access_key(user_id: &str) -> String {
    format!("auth:token:{}", user_id)
}

fn refresh_key(user_id: &str) -> String {
    format!("auth:refresh:{}", user_id)
}

async fn connection(
    redis_client: &redis::Client,
) -> Result<redis::aio::MultiplexedConnection, AppError> {
    redis_client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| AppError::AuthError(format!("Redis connection failed: {}", e)))
}

async fn set_with_ttl(
    redis_client: &redis::Client,
    key: &str,
    token: &str,
    ttl_secs: u64,
) -> Result<(), AppError> {
    let mut conn = connection(redis_client).await?;
    redis::cmd("SET")
        .arg(key)
        .arg(token)
        .arg("EX")
        .arg(ttl_secs)
        .query_async::<()>(&mut conn)
        .await
        .map_err(|e| AppError::AuthError(format!("Redis set failed: {}", e)))?;
    Ok(())
}

async fn get_key(redis_client: &redis::Client, key: &str) -> Result<Option<String>, AppError> {
    let mut conn = connection(redis_client).await?;
    let stored: Option<String> = redis::cmd("GET")
        .arg(key)
        .query_async(&mut conn)
        .await
        .map_err(|e| AppError::AuthError(format!("Redis get failed: {}", e)))?;
    Ok(stored)
}

async fn del_key(redis_client: &redis::Client, key: &str) -> Result<(), AppError> {
    let mut conn = connection(redis_client).await?;
    redis::cmd("DEL")
        .arg(key)
        .query_async::<()>(&mut conn)
        .await
        .map_err(|e| AppError::AuthError(format!("Redis del failed: {}", e)))?;
    Ok(())
}
