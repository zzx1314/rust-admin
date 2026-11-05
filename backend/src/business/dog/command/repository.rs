use crate::business::dog::command::domain::{
    CommandPageQuery, CreateCommandRequest, UpdateCommandRequest,
};
use crate::business::dog::command::entity::ActiveModel as CommandActiveModel;
use crate::business::dog::command::entity::Column as CommandColumn;
use crate::business::dog::command::entity::Entity as CommandEntity;
use crate::business::dog::command::entity::Model as DogCommand;
use crate::common::base::{BaseRepository, order_desc};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogCommandRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogCommandRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl BaseRepository for SeaOrmDogCommandRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmDogCommandRepository {
    pub async fn create(
        &self,
        req: &CreateCommandRequest,
        id: i64,
    ) -> Result<DogCommand, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        CommandEntity::insert(active_model)
            .exec(&*self.conn)
            .await?;
        CommandEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<DogCommand>, sea_orm::DbErr> {
        CommandEntity::find()
            .filter(CommandColumn::Id.eq(id))
            .filter(CommandColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_all_with_page(
        &self,
        query: &CommandPageQuery,
    ) -> Result<(Vec<DogCommand>, i64), sea_orm::DbErr> {
        let base_query = CommandEntity::find().filter(CommandColumn::IsDeleted.eq(0));
        let mut cond = Condition::all();
        if let Some(ref name) = query.command_name {
            cond = cond.add(CommandColumn::CommandName.contains(name.as_str()));
        }
        if let Some(ref ctype) = query.command_type {
            cond = cond.add(CommandColumn::CommandType.eq(ctype.as_str()));
        }
        if let Some(ref status) = query.status {
            cond = cond.add(CommandColumn::Status.eq(status.as_str()));
        }
        if let Some(ref device_id) = query.device_id {
            cond = cond.add(CommandColumn::DeviceId.eq(device_id.as_str()));
        }
        if let Some(task_id) = query.task_id {
            cond = cond.add(CommandColumn::TaskId.eq(task_id));
        }

        let total = base_query
            .clone()
            .filter(cond.clone())
            .count(&*self.conn)
            .await?;
        let current = query.current.max(1);
        let size = query.size.max(1);
        let offset = (current - 1) * size;

        let records = base_query
            .filter(cond)
            .order_by(CommandColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateCommandRequest,
    ) -> Result<Option<DogCommand>, sea_orm::DbErr> {
        let entity = CommandEntity::find()
            .filter(CommandColumn::Id.eq(id))
            .filter(CommandColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: CommandActiveModel = entity.into();
        if let Some(name) = &req.command_name {
            active_model.command_name = ActiveValue::set(name.clone());
        }
        if let Some(ctype) = &req.command_type {
            active_model.command_type = ActiveValue::set(Some(ctype.clone()));
        }
        if let Some(params) = &req.command_params {
            active_model.command_params = ActiveValue::set(Some(params.clone()));
        }
        if let Some(did) = &req.device_id {
            active_model.device_id = ActiveValue::set(Some(did.clone()));
        }
        if let Some(status) = &req.status {
            active_model.status = ActiveValue::set(status.clone());
        }
        if let Some(ret) = &req.return_value {
            active_model.return_value = ActiveValue::set(Some(ret.clone()));
        }
        if let Some(tid) = req.task_id {
            active_model.task_id = ActiveValue::set(Some(tid));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        CommandEntity::update(active_model)
            .exec(&*self.conn)
            .await?;
        self.find_by_id(id).await
    }

    pub async fn update_status(
        &self,
        id: i64,
        status: &str,
        return_value: Option<String>,
    ) -> Result<Option<DogCommand>, sea_orm::DbErr> {
        let command = self.find_by_id(id).await?;
        if let Some(mut command) = command {
            command.status = status.to_string();
            let ret = return_value.clone();
            command.return_value = return_value;
            command.update_time = Some(chrono::Utc::now());
            let mut active_model: CommandActiveModel = command.into();
            active_model.status = ActiveValue::Set(status.to_string());
            active_model.return_value = ActiveValue::Set(ret);
            CommandEntity::update(active_model)
                .exec(&*self.conn)
                .await?;
            self.find_by_id(id).await
        } else {
            Ok(None)
        }
    }

    pub async fn find_all(&self) -> Result<Vec<DogCommand>, sea_orm::DbErr> {
        CommandEntity::find()
            .filter(CommandColumn::IsDeleted.eq(0))
            .order_by(CommandColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let command = self.find_by_id(id).await?;
        if let Some(mut command) = command {
            command.is_deleted = 1;
            command.update_time = Some(chrono::Utc::now());
            let mut active_model: CommandActiveModel = command.into();
            active_model.is_deleted = ActiveValue::Set(1);
            CommandEntity::update(active_model)
                .exec(&*self.conn)
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
