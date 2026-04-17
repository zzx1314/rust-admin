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
        let redis_key = format!("auth:token:{}", user_id);
        let token = token.to_string();
        Box::pin(async move {
            let mut conn = redis_client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| AppError::AuthError(format!("Redis connection failed: {}", e)))?;

            redis::cmd("SET")
                .arg(&redis_key)
                .arg(&token)
                .arg("EX")
                .arg(ttl_secs)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| AppError::AuthError(format!("Redis set failed: {}", e)))?;

            Ok(())
        })
    }

    fn get_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let redis_client = self.redis_client.clone();
        let redis_key = format!("auth:token:{}", user_id);
        Box::pin(async move {
            let mut conn = redis_client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| AppError::AuthError(format!("Redis connection failed: {}", e)))?;

            let stored_token: Option<String> = redis::cmd("GET")
                .arg(&redis_key)
                .query_async(&mut conn)
                .await
                .map_err(|e| AppError::AuthError(format!("Redis get failed: {}", e)))?;

            Ok(stored_token)
        })
    }

    fn delete_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let redis_client = self.redis_client.clone();
        let redis_key = format!("auth:token:{}", user_id);
        Box::pin(async move {
            let mut conn = redis_client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| AppError::AuthError(format!("Redis connection failed: {}", e)))?;

            redis::cmd("DEL")
                .arg(&redis_key)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| AppError::AuthError(format!("Redis del failed: {}", e)))?;

            Ok(())
        })
    }
}
