use async_trait::async_trait;
use sea_orm::{Condition, DatabaseConnection, DbErr, Order};
use std::sync::Arc;

pub type RepoResult<T> = Result<T, DbErr>;
pub type RepoOptResult<T> = Result<Option<T>, DbErr>;
pub type RepoFuture<T> = Box<dyn Future<Output = RepoResult<T>> + Send>;

#[async_trait]
pub trait BaseRepository: Send + Sync {
    fn conn(&self) -> Arc<DatabaseConnection>;
}

pub trait RepoExt: BaseRepository {
    fn with_conn<F, R>(&self, f: F) -> R
    where
        F: FnOnce(Arc<DatabaseConnection>) -> R,
    {
        f(self.conn())
    }
}

impl<T: BaseRepository + ?Sized> RepoExt for T {}

#[derive(Clone)]
pub struct BaseRepo {
    conn: Arc<DatabaseConnection>,
}

impl BaseRepo {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }

    pub fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }

    pub fn conn_ref(&self) -> &Arc<DatabaseConnection> {
        &self.conn
    }
}

pub fn make_condition() -> Condition {
    Condition::all()
}

pub fn order_desc() -> Order {
    Order::Desc
}

pub fn order_asc() -> Order {
    Order::Asc
}

#[macro_export]
macro_rules! soft_delete_filter {
    ($col:ident) => {
        $col.eq(0)
    };
}

#[macro_export]
macro_rules! impl_repo_conn {
    ($struct:ident) => {
        impl $crate::common::base::BaseRepository for $struct {
            fn conn(&self) -> std::sync::Arc<sea_orm::DatabaseConnection> {
                self.conn.clone()
            }
        }
    };
}
