use crate::business::dog::command::repository::SeaOrmDogCommandRepository;
use crate::business::dog::device::repository::SeaOrmDogDeviceRepository;
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::business::dog::model::repository::SeaOrmDogModelRepository;
use crate::business::dog::route_library::repository::SeaOrmDogRouteLibraryRepository;
use crate::business::dog::task::repository::SeaOrmDogTaskRepository;
use crate::common::data_scope::DataScope;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize, Clone)]
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

/// Per-weekday creation counts for the previous and the current week.
///
/// Weeks run Monday..Sunday and follow Beijing time (UTC+8), matching the
/// weekday labels rendered by the dashboard.
#[derive(Debug, Default, Clone, Copy)]
struct DailyCounts {
    previous: [i64; 7],
    current: [i64; 7],
}

impl DailyCounts {
    fn from_times(times: &[DateTime<Utc>], current_week_start: NaiveDate) -> Self {
        let mut counts = Self::default();
        for time in times {
            let offset = beijing_date(*time)
                .signed_duration_since(current_week_start)
                .num_days();
            if (0..7).contains(&offset) {
                counts.current[offset as usize] += 1;
            } else if (-7..0).contains(&offset) {
                counts.previous[(offset + 7) as usize] += 1;
            }
        }
        counts
    }

    fn current_week(&self) -> Vec<i64> {
        self.current.to_vec()
    }

    fn previous_week(&self) -> Vec<i64> {
        self.previous.to_vec()
    }

    fn current_total(&self) -> i64 {
        self.current.iter().sum()
    }

    fn previous_total(&self) -> i64 {
        self.previous.iter().sum()
    }
}

/// Beijing (UTC+8) offset used for day/week boundaries.
fn beijing_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8 offset is valid")
}

fn beijing_date(dt: DateTime<Utc>) -> NaiveDate {
    dt.with_timezone(&beijing_offset()).date_naive()
}

/// Monday of the week containing `date`.
fn week_start_of(date: NaiveDate) -> NaiveDate {
    date - Duration::days(i64::from(date.weekday().num_days_from_monday()))
}

/// The UTC instant for 00:00 Beijing time on `date`.
fn start_of_beijing_day(date: NaiveDate) -> DateTime<Utc> {
    beijing_offset()
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight is valid"))
        .single()
        .expect("UTC+8 has no DST transitions")
        .with_timezone(&Utc)
}

/// Week-over-week growth rendered the way the dashboard card shows it.
fn growth_percent(current: i64, previous: i64) -> String {
    if previous == 0 {
        return if current == 0 {
            "0%".to_string()
        } else {
            "+100%".to_string()
        };
    }
    let percent = (current - previous) as f64 * 100.0 / previous as f64;
    if percent >= 0.0 {
        format!("+{}%", percent.round() as i64)
    } else {
        format!("{}%", percent.round() as i64)
    }
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

    /// Monday of the current and of the previous week (Beijing time).
    fn week_anchors() -> (NaiveDate, NaiveDate) {
        let current_start = week_start_of(beijing_date(Utc::now()));
        (current_start, current_start - Duration::days(7))
    }

    fn summary(
        name: &str,
        value: i64,
        daily: DailyCounts,
        bg_color: &str,
        color: &str,
        duration: i64,
    ) -> StatisticsSummary {
        StatisticsSummary {
            name: name.to_string(),
            value,
            percent: growth_percent(daily.current_total(), daily.previous_total()),
            data: daily.current_week(),
            bg_color: bg_color.to_string(),
            color: color.to_string(),
            duration,
        }
    }

    pub async fn get_all_statistics(
        &self,
        scope: &DataScope,
    ) -> Result<Vec<StatisticsSummary>, sea_orm::DbErr> {
        let (current_start, previous_start) = Self::week_anchors();
        let since = start_of_beijing_day(previous_start);

        let device_count = self.device_repo.count(scope).await?;
        let task_count = self.task_repo.count(scope).await?;
        let model_count = self.model_repo.count().await?;
        let route_count = self.route_repo.count().await?;

        let devices = DailyCounts::from_times(
            &self
                .device_repo
                .find_create_times_since(since, scope)
                .await?,
            current_start,
        );
        let tasks = DailyCounts::from_times(
            &self.task_repo.find_create_times_since(since, scope).await?,
            current_start,
        );
        let models = DailyCounts::from_times(
            &self.model_repo.find_create_times_since(since).await?,
            current_start,
        );
        let routes = DailyCounts::from_times(
            &self.route_repo.find_create_times_since(since).await?,
            current_start,
        );

        Ok(vec![
            Self::summary(
                "设备数量",
                device_count,
                devices,
                "#effaff",
                "#41b6ff",
                2200,
            ),
            Self::summary("任务数量", task_count, tasks, "#fff5f4", "#e85f33", 1600),
            Self::summary("模型数量", model_count, models, "#eff8f4", "#26ce83", 1500),
            Self::summary("路线数量", route_count, routes, "#f6f4fe", "#7846e5", 100),
        ])
    }

    /// Index 0 is the previous week, index 1 the current week.
    pub async fn get_weekly_statistics(&self) -> Result<Vec<WeeklyStatistics>, sea_orm::DbErr> {
        let (current_start, previous_start) = Self::week_anchors();
        let since = start_of_beijing_day(previous_start);
        // The weekly chart is a global overview, so no user scope is applied.
        let global_scope = DataScope::new(0, true);

        let devices = DailyCounts::from_times(
            &self
                .device_repo
                .find_create_times_since(since, &global_scope)
                .await?,
            current_start,
        );
        let tasks = DailyCounts::from_times(
            &self
                .task_repo
                .find_create_times_since(since, &global_scope)
                .await?,
            current_start,
        );
        let models = DailyCounts::from_times(
            &self.model_repo.find_create_times_since(since).await?,
            current_start,
        );
        let routes = DailyCounts::from_times(
            &self.route_repo.find_create_times_since(since).await?,
            current_start,
        );

        Ok(vec![
            WeeklyStatistics {
                device_count: devices.previous_week(),
                model_count: models.previous_week(),
                route_count: routes.previous_week(),
                task_count: tasks.previous_week(),
            },
            WeeklyStatistics {
                device_count: devices.current_week(),
                model_count: models.current_week(),
                route_count: routes.current_week(),
                task_count: tasks.current_week(),
            },
        ])
    }

    /// Device counts grouped by `device_type` (largest group first).
    pub async fn get_device_type_statistics(
        &self,
        scope: &DataScope,
    ) -> Result<serde_json::Value, sea_orm::DbErr> {
        let devices = self.device_repo.find_all(scope).await?;

        let mut counts: HashMap<String, i64> = HashMap::new();
        for device in devices {
            let device_type = device
                .device_type
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "未知".to_string());
            *counts.entry(device_type).or_insert(0) += 1;
        }

        let mut grouped: Vec<(String, i64)> = counts.into_iter().collect();
        grouped.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let (xdata, ydata): (Vec<String>, Vec<i64>) = grouped.into_iter().unzip();

        Ok(serde_json::json!({
            "xdata": xdata,
            "ydata": ydata
        }))
    }
}
