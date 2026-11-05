use crate::business::dog::command::repository::SeaOrmDogCommandRepository;
use crate::business::dog::device::repository::SeaOrmDogDeviceRepository;
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::business::dog::model::repository::SeaOrmDogModelRepository;
use crate::business::dog::route_library::repository::SeaOrmDogRouteLibraryRepository;
use crate::business::dog::task::repository::SeaOrmDogTaskRepository;
use crate::common::data_scope::DataScope;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct StatisticsSummary {
    pub name: String,
    pub value: i64,
    pub percent: String,
    pub data: Vec<i64>,
    pub bg_color: String,
    pub color: String,
    pub duration: i64,
}

#[derive(Debug, Serialize)]
pub struct WeeklyStatistics {
    pub device_count: Vec<i64>,
    pub model_count: Vec<i64>,
    pub route_count: Vec<i64>,
    pub task_count: Vec<i64>,
}

pub struct DogStatisticsService {
    device_repo: SeaOrmDogDeviceRepository,
    task_repo: SeaOrmDogTaskRepository,
    model_repo: SeaOrmDogModelRepository,
    route_repo: SeaOrmDogRouteLibraryRepository,
    _media_repo: SeaOrmDogMediaRepository,
    _command_repo: SeaOrmDogCommandRepository,
}

impl DogStatisticsService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device_repo: SeaOrmDogDeviceRepository,
        task_repo: SeaOrmDogTaskRepository,
        model_repo: SeaOrmDogModelRepository,
        route_repo: SeaOrmDogRouteLibraryRepository,
        media_repo: SeaOrmDogMediaRepository,
        command_repo: SeaOrmDogCommandRepository,
    ) -> Self {
        Self {
            device_repo,
            task_repo,
            model_repo,
            route_repo,
            _media_repo: media_repo,
            _command_repo: command_repo,
        }
    }

    pub async fn get_all_statistics(
        &self,
        scope: &DataScope,
    ) -> Result<Vec<StatisticsSummary>, sea_orm::DbErr> {
        let device_count = self.device_repo.count(scope).await?;
        let task_count = self.task_repo.find_all(scope).await?.len() as i64;
        let model_count = self.model_repo.find_all().await?.len() as i64;
        let route_count = self.route_repo.find_all().await?.len() as i64;

        Ok(vec![
            StatisticsSummary {
                name: "设备数量".to_string(),
                value: device_count,
                percent: "+100%".to_string(),
                data: vec![device_count],
                bg_color: "#effaff".to_string(),
                color: "#41b6ff".to_string(),
                duration: 2200,
            },
            StatisticsSummary {
                name: "任务数量".to_string(),
                value: task_count,
                percent: "+100%".to_string(),
                data: vec![task_count],
                bg_color: "#fff5f4".to_string(),
                color: "#e85f33".to_string(),
                duration: 1600,
            },
            StatisticsSummary {
                name: "模型数量".to_string(),
                value: model_count,
                percent: "+100%".to_string(),
                data: vec![model_count],
                bg_color: "#eff8f4".to_string(),
                color: "#26ce83".to_string(),
                duration: 1500,
            },
            StatisticsSummary {
                name: "路线数量".to_string(),
                value: route_count,
                percent: "+100%".to_string(),
                data: vec![route_count],
                bg_color: "#f6f4fe".to_string(),
                color: "#7846e5".to_string(),
                duration: 100,
            },
        ])
    }

    pub async fn get_weekly_statistics(&self) -> Result<Vec<WeeklyStatistics>, sea_orm::DbErr> {
        Ok(vec![
            WeeklyStatistics {
                device_count: vec![0, 0, 0, 0, 0, 0, 0],
                model_count: vec![0, 0, 0, 0, 0, 0, 0],
                route_count: vec![0, 0, 0, 0, 0, 0, 0],
                task_count: vec![0, 0, 0, 0, 0, 0, 0],
            },
            WeeklyStatistics {
                device_count: vec![0, 0, 0, 0, 0, 0, 0],
                model_count: vec![0, 0, 0, 0, 0, 0, 0],
                route_count: vec![0, 0, 0, 0, 0, 0, 0],
                task_count: vec![0, 0, 0, 0, 0, 0, 0],
            },
        ])
    }

    pub async fn get_device_type_statistics(&self) -> Result<serde_json::Value, sea_orm::DbErr> {
        Ok(serde_json::json!({
            "xdata": [],
            "ydata": []
        }))
    }
}
