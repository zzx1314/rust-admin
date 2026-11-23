use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{
    Shift, ShiftCopyRequest, ShiftCopyResult, ShiftCopySkip, ShiftDuplicateRequest, ShiftQuery,
    ShiftRequest, ShiftSlot, ShiftSlotRequest, Staff, StaffImportResult, StaffImportRow,
    StaffQuery, StaffRequest,
};
use axum::Extension;
use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use chrono::{Duration, NaiveDate, NaiveTime};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Sqlite, SqlitePool, Transaction};
use std::collections::HashSet;

/// 排班状态（复制时允许改写成其中一种）
const SHIFT_STATUSES: [&str; 3] = ["已发布", "待确认", "草案"];

/// Excel 导入时给新责任人轮换的头像底色（周视图的圆形头像用它上色）
const STAFF_COLORS: [&str; 6] = [
    "#0284c7", "#7c3aed", "#0f766e", "#b45309", "#be123c", "#4d7c0f",
];

/// 班次定义表（早 / 中 / 夜是迁移插入的默认行，可在「班次设置」里自定义）
const SHIFT_SLOT_SELECT: &str = "SELECT ss.id, ss.name, ss.start_time, ss.end_time,
    ss.start_time || ' - ' || ss.end_time AS time_range, ss.color, ss.sort_order,
    ss.active, ss.note,
    (SELECT COUNT(*) FROM shifts s WHERE s.slot = ss.name) AS shift_count,
    ss.created_at, ss.updated_at
    FROM shift_slots ss";

const STAFF_SELECT: &str = "SELECT st.id, st.staff_code, st.name, st.role, st.team, st.phone,
    st.color, st.status, st.skills, st.user_id, u.username AS user_name,
    st.created_at, st.updated_at,
    (SELECT COUNT(*) FROM shifts s
      WHERE s.staff_id = st.id
        AND (?1 IS NULL OR s.shift_date >= ?1)
        AND (?2 IS NULL OR s.shift_date <= ?2)) AS shift_count,
    (SELECT COUNT(*) FROM work_orders w
      WHERE w.staff_id = st.id AND w.status <> '已完成') AS open_order_count
    FROM staff st
    LEFT JOIN users u ON u.id = st.user_id";

const SHIFT_SELECT: &str = "SELECT s.id, s.shift_code, s.shift_date, s.slot, s.time_range,
    s.staff_id, s.owner_name, st.role AS staff_role, st.team AS staff_team,
    st.color AS staff_color, s.park_id, p.park_name, s.zone_id, z.zone_name,
    s.task_id, s.task_name,
    (SELECT COUNT(*) FROM devices d
      WHERE s.park_id IS NOT NULL AND d.park_id = s.park_id
        AND (s.zone_id IS NULL OR d.zone_id = s.zone_id)) AS device_count,
    s.status, s.note, s.created_at, s.updated_at, s.created_by
    FROM shifts s
    LEFT JOIN staff st ON st.id = s.staff_id
    LEFT JOIN parks p ON p.id = s.park_id
    LEFT JOIN zones z ON z.id = s.zone_id";
mod copy;
mod shift_slots;
mod shifts;
mod staff;

// 路由入口（`app.rs` 用）
pub use copy::copy_shifts;
pub use shift_slots::{create_shift_slot, delete_shift_slot, list_shift_slots, update_shift_slot};
pub use shifts::{
    create_shift, delete_shift, duplicate_shift, get_shift, list_shifts, shift_stats, update_shift,
};
pub use staff::{
    create_staff, delete_staff, import_staff, list_staff, staff_import_template, update_staff,
};

// crate 内共享：子模块之间、以及工单模块（`staff_id_by_name`）用的 helper。
#[allow(clippy::wildcard_imports)]
pub(crate) use copy::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use shift_slots::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use staff::*;
