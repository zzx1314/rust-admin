use crate::business::dog::map::domain::{
    CreateMapRequest, DogMap, MapPageQuery, MapVO, UpdateMapRequest, UploadMapRequest,
};
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::business::dog::park::repository::SeaOrmDogParkRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::generate_id;

pub struct DogMapService {
    repo: SeaOrmDogMapRepository,
    park_repo: SeaOrmDogParkRepository,
}

impl DogMapService {
    pub fn new(repo: SeaOrmDogMapRepository, park_repo: SeaOrmDogParkRepository) -> Self {
        Self { repo, park_repo }
    }

    pub async fn create(
        &self,
        req: CreateMapRequest,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        if let Some(park_id) = req.park_id {
            self.ensure_park_access(park_id, scope).await?;
        }
        let id = generate_id();
        self.repo
            .create(&req, id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: MapPageQuery,
        scope: &DataScope,
    ) -> Result<PageResponse<MapVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(MapVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self, scope: &DataScope) -> Result<Vec<DogMap>, AppError> {
        self.repo
            .find_all(scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateMapRequest,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        if let Some(park_id) = req.park_id {
            self.ensure_park_access(park_id, scope).await?;
        }
        self.repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!("Map with id {} not found", id)));
        }
        Ok(())
    }

    /// Stores an uploaded `.zip` map package and creates/updates the map row.
    pub async fn save_upload(
        &self,
        info: UploadMapRequest,
        original_name: &str,
        bytes: &[u8],
        upload_root: &std::path::Path,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        let file_name = std::path::Path::new(original_name)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "map.zip".to_string());
        let file_format = std::path::Path::new(&file_name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if file_format != "zip" {
            return Err(AppError::BadRequest("地图包仅支持 .zip 格式".to_string()));
        }
        if info.map_name.trim().is_empty() {
            return Err(AppError::BadRequest("地图名称不能为空".to_string()));
        }

        let dir = upload_root.join("maps");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| AppError::InternalError(format!("创建地图目录失败: {error}")))?;
        let stored_name = format!("{}-{}", generate_id(), file_name);
        tokio::fs::write(dir.join(&stored_name), bytes)
            .await
            .map_err(|error| AppError::InternalError(format!("保存地图包失败: {error}")))?;

        let file_path = format!("maps/{stored_name}");
        let file_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);

        match info.id {
            Some(id) => {
                let req = UpdateMapRequest {
                    map_name: Some(info.map_name),
                    map_type: info.map_type,
                    map_data: None,
                    park_id: info.park_id,
                    file_name: Some(file_name),
                    file_path: Some(file_path),
                    file_format: Some(file_format),
                    file_size: Some(file_size),
                    remarks: info.remarks,
                };
                self.update(id, req, scope).await
            }
            None => {
                let req = CreateMapRequest {
                    map_name: info.map_name,
                    map_type: info.map_type,
                    map_data: None,
                    park_id: info.park_id,
                    file_name: Some(file_name),
                    file_path: Some(file_path),
                    file_format: Some(file_format),
                    file_size: Some(file_size),
                    remarks: info.remarks,
                };
                self.create(req, scope).await
            }
        }
    }

    async fn ensure_park_access(&self, park_id: i64, scope: &DataScope) -> Result<(), AppError> {
        let park = self
            .park_repo
            .find_by_id(park_id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if park.is_none() {
            return Err(AppError::BadRequest(format!(
                "Park with id {} not found or not accessible",
                park_id
            )));
        }
        Ok(())
    }
}
