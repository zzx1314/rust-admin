use crate::business::dog::command::repository::SeaOrmDogCommandRepository;
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::business::dog::model::repository::SeaOrmDogModelRepository;
use crate::business::dog::services::repository::SeaOrmDogServicesRepository;
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
    pub model_count: Vec<i64>,
    pub media_count: Vec<i64>,
    pub command_count: Vec<i64>,
    pub services_count: Vec<i64>,
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
    model_repo: SeaOrmDogModelRepository,
    media_repo: SeaOrmDogMediaRepository,
    command_repo: SeaOrmDogCommandRepository,
    services_repo: SeaOrmDogServicesRepository,
}

impl DogStatisticsService {
    pub fn new(
        model_repo: SeaOrmDogModelRepository,
        media_repo: SeaOrmDogMediaRepository,
        command_repo: SeaOrmDogCommandRepository,
        services_repo: SeaOrmDogServicesRepository,
    ) -> Self {
        Self {
            model_repo,
            media_repo,
            command_repo,
            services_repo,
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

        let model_count = self.model_repo.count().await?;
        let media_count = self.media_repo.count().await?;
        let command_count = self.command_repo.count().await?;
        let services_count = self.services_repo.count().await?;

        let models = DailyCounts::from_times(
            &self.model_repo.find_create_times_since(since).await?,
            current_start,
        );
        let media = DailyCounts::from_times(
            &self.media_repo.find_create_times_since(since).await?,
            current_start,
        );
        let commands = DailyCounts::from_times(
            &self.command_repo.find_create_times_since(since).await?,
            current_start,
        );
        let services = DailyCounts::from_times(
            &self.services_repo.find_create_times_since(since).await?,
            current_start,
        );

        Ok(vec![
            Self::summary("模型数量", model_count, models, "#eff8f4", "#26ce83", 1500),
            Self::summary("媒体数量", media_count, media, "#effaff", "#41b6ff", 2000),
            Self::summary(
                "指令数量",
                command_count,
                commands,
                "#fff5f4",
                "#e85f33",
                1600,
            ),
            Self::summary(
                "服务数量",
                services_count,
                services,
                "#f6f4fe",
                "#7846e5",
                100,
            ),
        ])
    }

    /// Index 0 is the previous week, index 1 the current week.
    pub async fn get_weekly_statistics(&self) -> Result<Vec<WeeklyStatistics>, sea_orm::DbErr> {
        let (current_start, previous_start) = Self::week_anchors();
        let since = start_of_beijing_day(previous_start);

        let models = DailyCounts::from_times(
            &self.model_repo.find_create_times_since(since).await?,
            current_start,
        );
        let media = DailyCounts::from_times(
            &self.media_repo.find_create_times_since(since).await?,
            current_start,
        );
        let commands = DailyCounts::from_times(
            &self.command_repo.find_create_times_since(since).await?,
            current_start,
        );
        let services = DailyCounts::from_times(
            &self.services_repo.find_create_times_since(since).await?,
            current_start,
        );

        Ok(vec![
            WeeklyStatistics {
                model_count: models.previous_week(),
                media_count: media.previous_week(),
                command_count: commands.previous_week(),
                services_count: services.previous_week(),
            },
            WeeklyStatistics {
                model_count: models.current_week(),
                media_count: media.current_week(),
                command_count: commands.current_week(),
                services_count: services.current_week(),
            },
        ])
    }
}
