use crate::business::dog::config::repository::SeaOrmDogConfigRepository;
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::business::dog::model::repository::SeaOrmDogModelRepository;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc};
use serde::Serialize;

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
    pub map_count: Vec<i64>,
    pub model_count: Vec<i64>,
    pub media_count: Vec<i64>,
    pub config_count: Vec<i64>,
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
    map_repo: SeaOrmDogMapRepository,
    model_repo: SeaOrmDogModelRepository,
    media_repo: SeaOrmDogMediaRepository,
    config_repo: SeaOrmDogConfigRepository,
}

impl DogStatisticsService {
    pub fn new(
        map_repo: SeaOrmDogMapRepository,
        model_repo: SeaOrmDogModelRepository,
        media_repo: SeaOrmDogMediaRepository,
        config_repo: SeaOrmDogConfigRepository,
    ) -> Self {
        Self {
            map_repo,
            model_repo,
            media_repo,
            config_repo,
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

    pub async fn get_all_statistics(&self) -> Result<Vec<StatisticsSummary>, sea_orm::DbErr> {
        let (current_start, previous_start) = Self::week_anchors();
        let since = start_of_beijing_day(previous_start);

        let map_count = self.map_repo.count().await?;
        let model_count = self.model_repo.count().await?;
        let media_count = self.media_repo.count().await?;
        let config_count = self.config_repo.count().await?;

        let maps = DailyCounts::from_times(
            &self.map_repo.find_create_times_since(since).await?,
            current_start,
        );
        let models = DailyCounts::from_times(
            &self.model_repo.find_create_times_since(since).await?,
            current_start,
        );
        let media = DailyCounts::from_times(
            &self.media_repo.find_create_times_since(since).await?,
            current_start,
        );
        let configs = DailyCounts::from_times(
            &self.config_repo.find_create_times_since(since).await?,
            current_start,
        );

        Ok(vec![
            Self::summary("地图数量", map_count, maps, "#eef6ff", "#3b82f6", 2200),
            Self::summary("模型数量", model_count, models, "#eff8f4", "#26ce83", 1500),
            Self::summary("媒体数量", media_count, media, "#effaff", "#41b6ff", 2000),
            Self::summary("参数配置", config_count, configs, "#f6f4fe", "#7846e5", 100),
        ])
    }

    /// Index 0 is the previous week, index 1 the current week.
    pub async fn get_weekly_statistics(&self) -> Result<Vec<WeeklyStatistics>, sea_orm::DbErr> {
        let (current_start, previous_start) = Self::week_anchors();
        let since = start_of_beijing_day(previous_start);

        let maps = DailyCounts::from_times(
            &self.map_repo.find_create_times_since(since).await?,
            current_start,
        );
        let models = DailyCounts::from_times(
            &self.model_repo.find_create_times_since(since).await?,
            current_start,
        );
        let media = DailyCounts::from_times(
            &self.media_repo.find_create_times_since(since).await?,
            current_start,
        );
        let configs = DailyCounts::from_times(
            &self.config_repo.find_create_times_since(since).await?,
            current_start,
        );

        Ok(vec![
            WeeklyStatistics {
                map_count: maps.previous_week(),
                model_count: models.previous_week(),
                media_count: media.previous_week(),
                config_count: configs.previous_week(),
            },
            WeeklyStatistics {
                map_count: maps.current_week(),
                model_count: models.current_week(),
                media_count: media.current_week(),
                config_count: configs.current_week(),
            },
        ])
    }
}
