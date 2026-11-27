use crate::app::AppState;
use crate::auth::{self, Claims};
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{Park, ParkQuery, ParkRequest};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use sqlx::QueryBuilder;

const PARK_SELECT: &str =
    "SELECT p.id, p.park_code, p.park_name, p.park_description, p.park_address,
    p.area, p.owner_name, p.park_type, p.status, p.color, p.created_at, p.updated_at,
    p.map_version_id, p.map_status, p.created_by,
    (SELECT COUNT(*) FROM zones z WHERE z.park_id = p.id) AS zone_count,
    (SELECT COUNT(*) FROM devices d WHERE d.park_id = p.id) AS device_count,
    (SELECT COUNT(*) FROM tasks t WHERE t.park_id = p.id) AS task_count,
    (SELECT COUNT(*) FROM tasks t WHERE t.park_id = p.id AND t.task_status = '已完成') AS done_count
    FROM parks p";

pub async fn list_parks(
    State(state): State<AppState>,
    Query(query): Query<ParkQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<Park>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(PARK_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(q) = query.q.filter(|value| !value.trim().is_empty()) {
        let keyword = format!("%{}%", q.trim());
        builder
            .push(" AND (p.park_name LIKE ")
            .push_bind(keyword.clone())
            .push(" OR p.park_code LIKE ")
            .push_bind(keyword.clone())
            .push(" OR p.owner_name LIKE ")
            .push_bind(keyword)
            .push(")");
    }
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND p.status = ").push_bind(status);
    }
    // 非管理员只能看到自己创建的园区
    if !claims.is_admin {
        builder.push(" AND p.created_by = ").push_bind(claims.sub);
    }
    builder.push(" ORDER BY p.id ASC");

    let parks = builder
        .build_query_as::<Park>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(parks))
}

pub async fn get_park(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Park>>, AppError> {
    let park = fetch_park(&state, id).await?;
    ensure_owner(&park, &claims)?;
    Ok(ApiResponse::ok(park))
}

pub async fn create_park(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ParkRequest>,
) -> Result<Json<ApiResponse<Park>>, AppError> {
    if payload.park_name.trim().is_empty() {
        return Err(AppError::BadRequest("园区名称不能为空".to_string()));
    }
    let next_id = crate::ids::next_id(&state.db, "parks", "parks").await?;
    let park_code = payload
        .park_code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("PK-{next_id:03}"));
    let timestamp = now();

    sqlx::query(
        "INSERT INTO parks (park_code, park_name, park_description, park_address, area, owner_name,
            park_type, status, color, created_at, updated_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(&park_code)
    .bind(payload.park_name.trim())
    .bind(payload.park_description)
    .bind(payload.park_address)
    .bind(payload.area.unwrap_or(0.0))
    .bind(payload.owner_name)
    .bind(payload.park_type)
    .bind(payload.status.unwrap_or_else(|| "运行中".to_string()))
    .bind(payload.color.unwrap_or_else(|| "#0284c7".to_string()))
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(claims.sub)
    .execute(&state.db)
    .await?;

    let id: i64 = sqlx::query_scalar("SELECT id FROM parks WHERE park_code = ?1")
        .bind(&park_code)
        .fetch_one(&state.db)
        .await?;
    // 建档时选的地图基线：地图跟着设备走，设备地图默认 `park_id IS NULL`，
    // 这里把它认领到新园区，园区才会真正拿到「当前生效基线」。
    if let Some((map_version_id, map_status)) =
        resolve_park_map(&state, id, payload.map_version_id, &claims, None).await?
        && map_version_id.is_some()
    {
        sqlx::query(
            "UPDATE parks SET map_version_id = ?1, map_status = ?2, updated_at = ?3 WHERE id = ?4",
        )
        .bind(&map_version_id)
        .bind(&map_status)
        .bind(&timestamp)
        .bind(id)
        .execute(&state.db)
        .await?;
    }
    Ok(ApiResponse::ok(fetch_park(&state, id).await?))
}

pub async fn update_park(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ParkRequest>,
) -> Result<Json<ApiResponse<Park>>, AppError> {
    let existing = fetch_park(&state, id).await?;
    if !claims.is_admin
        && existing
            .created_by
            .map(|uid| uid != claims.sub)
            .unwrap_or(true)
    {
        return Err(AppError::NotFound("园区不存在".to_string()));
    }
    let (map_version_id, map_status) = resolve_park_map(
        &state,
        id,
        payload.map_version_id,
        &claims,
        existing.map_version_id.as_deref(),
    )
    .await?
    .unwrap_or((existing.map_version_id, existing.map_status));
    sqlx::query(
        "UPDATE parks SET park_name = ?1, park_description = ?2, park_address = ?3, area = ?4,
            owner_name = ?5, park_type = ?6, status = ?7, color = ?8, map_version_id = ?9,
            map_status = ?10, updated_at = ?11
         WHERE id = ?12",
    )
    .bind(if payload.park_name.trim().is_empty() {
        existing.park_name
    } else {
        payload.park_name.trim().to_string()
    })
    .bind(payload.park_description.or(existing.park_description))
    .bind(payload.park_address.or(existing.park_address))
    .bind(payload.area.unwrap_or(existing.area))
    .bind(payload.owner_name.or(existing.owner_name))
    .bind(payload.park_type.or(existing.park_type))
    .bind(payload.status.unwrap_or(existing.status))
    .bind(payload.color.unwrap_or(existing.color))
    .bind(map_version_id)
    .bind(map_status)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_park(&state, id).await?))
}

pub async fn delete_park(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let park = fetch_park(&state, id).await?;
    if !claims.is_admin && park.created_by.map(|uid| uid != claims.sub).unwrap_or(true) {
        return Err(AppError::NotFound("园区不存在".to_string()));
    }
    let mut transaction = state.db.begin().await?;
    sqlx::query("DELETE FROM zones WHERE park_id = ?1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE devices SET park_id = NULL, zone_id = NULL,
            status = CASE WHEN status = '离线' THEN '离线' ELSE '待绑定' END
         WHERE park_id = ?1",
    )
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE tasks SET park_id = NULL, zone_id = NULL WHERE park_id = ?1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    let result = sqlx::query("DELETE FROM parks WHERE id = ?1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

/// 非管理员只能操作自己创建的资源。返回 `NotFound` 而非 `Forbidden`，
/// 避免暴露「该记录由他人创建」的信息。
pub(crate) fn ensure_owner<T: OwnerCheck>(resource: &T, claims: &Claims) -> Result<(), AppError> {
    if claims.is_admin {
        return Ok(());
    }
    match resource.creator_id() {
        Some(uid) if uid == claims.sub => Ok(()),
        _ => Err(AppError::NotFound("记录不存在".to_string())),
    }
}

/// 让多种资源类型可以共用 `ensure_owner`
pub(crate) trait OwnerCheck {
    fn creator_id(&self) -> Option<i64>;
}

impl OwnerCheck for Park {
    fn creator_id(&self) -> Option<i64> {
        self.created_by
    }
}

async fn fetch_park(state: &AppState, id: i64) -> Result<Park, AppError> {
    let sql = format!("{PARK_SELECT} WHERE p.id = ?1");
    sqlx::query_as::<_, Park>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("园区不存在".to_string()))
}

/// 解析园区表单提交的地图基线绑定。
/// 返回 `None` 表示请求未携带该字段，保留原值；`Some((None, status))` 表示解绑。
///
/// 地图先跟设备、再跟园区：设备刚回传的地图 `park_id IS NULL`，建档时被选中的那张
/// 在这里被「认领」到本园区。
///
/// **不同园区可以绑定同一张地图基线**（`parks.map_version_id` 各指各的，
/// `map_versions.park_id` 只记采集 / 主归属园区，不再独占）：同一张图被第二个园区选中
/// 时不改归属，只校验调用者对这张图有没有使用权，见 [`ensure_map_usable`]。
/// 历史 / 演示地图（没有 `device_code`、没有上传人）只给管理员用，
/// 免得随手把演示园区的基线挂到业务园区上。
///
/// **只认中台已发布的地图**：状态由中台统一维护（业务库只照搬），能当园区基线的
/// 就是「中台已发布」那一张。未发布（草稿 / 部分标定）只能在园区里维持现状
/// （`current` 就是这张，比如中台把某张图退回草稿），换绑 / 新绑一律拒绝，
/// 否则会再次出现「业务平台基线指向一张中台未发布 / 已删除的图」。
pub(crate) async fn resolve_park_map(
    state: &AppState,
    park_id: i64,
    requested: Option<String>,
    claims: &Claims,
    current: Option<&str>,
) -> Result<Option<(Option<String>, String)>, AppError> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let requested = requested.trim();
    if requested.is_empty() {
        return Ok(Some((None, "待标定".to_string())));
    }
    if requested == "__none__" {
        return Ok(Some((None, "未绑定".to_string())));
    }
    let map = sqlx::query_as::<_, (Option<i64>, String, Option<String>, Option<i64>)>(
        "SELECT park_id, status, device_code, owner_user_id FROM map_versions WHERE id = ?1",
    )
    .bind(requested)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::BadRequest(format!("地图基线 {requested} 不存在")))?;
    // 先判「你能不能碰这张图」再判状态：没权限的人得到的仍是「无法绑定」，
    // 不会因为图恰好未发布而把授权问题的原因盖掉。
    if let Some(owner) = map.0
        && owner != park_id
    {
        ensure_map_usable(state, claims, requested, map.2.as_deref(), map.3).await?;
    }
    // 未发布的地图只能「保持现状」：编辑园区时前端会把当前基线原样提交回来，
    // 那一次不能把人卡住；真正换绑时一律要求已发布。
    if map.1.trim() != "已发布" && current != Some(requested) {
        return Err(AppError::BadRequest(format!(
            "地图基线 {requested} 当前状态为 {}：地图状态由中台统一管理，\
             只有中台「已发布」的地图能绑定为园区基线（请先在中台发布）",
            map.1
        )));
    }
    match map.0 {
        Some(owner) if owner != park_id => {
            // 共用：归属不动，只确认这张图你可以用（授权已在上面做过）。
            tracing::info!("园区 {park_id} 共用设备地图 {requested}（原归属园区 {owner}）");
        }
        // 设备地图还没归属园区：绑定动作就是认领
        None => {
            sqlx::query("UPDATE map_versions SET park_id = ?1 WHERE id = ?2")
                .bind(park_id)
                .bind(requested)
                .execute(&state.db)
                .await?;
            tracing::info!("设备地图 {requested} 已归属园区 {park_id}");
        }
        Some(_) => {}
    }
    let status = if map.1 == "部分标定" {
        "部分标定".to_string()
    } else if map.1 == "已发布" {
        "已标定".to_string()
    } else {
        // 现状基线（中台退回草稿 / 已归档）：园区徽章跟着地图状态走，不要假标成已标定
        map.1.trim().to_string()
    };
    Ok(Some((Some(requested.to_string()), status)))
}

/// 「把别的园区的设备地图绑到本园区」的授权。
///
/// 能用的三种人：管理员；地图上传人本人（`owner_user_id`）；该地图采集设备在
/// 当前账号的中台分配关系里（设备页「绑定园区」那批人）。
/// 没有 `device_code` 也没有上传人的历史 / 演示地图一律只给管理员，
/// 否则任何运营账号都能把演示园区的基线挂到自己园区上。
async fn ensure_map_usable(
    state: &AppState,
    claims: &Claims,
    map_id: &str,
    device_code: Option<&str>,
    owner_user_id: Option<i64>,
) -> Result<(), AppError> {
    if claims.is_admin {
        return Ok(());
    }
    if let (Some(owner), Some(me)) = (owner_user_id, claims.zentai_user_id)
        && owner == me
    {
        return Ok(());
    }
    let denied = || {
        AppError::BadRequest(format!(
            "地图基线 {map_id} 属于其他园区，且采集设备不在你的管辖范围内，无法绑定"
        ))
    };
    let Some(device_code) = device_code.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(denied());
    };
    // 本地运营账号没有中台身份 → `None`，不能当成「全部设备都归我」放行。
    // 中台不可达时也按「无权」处理：报警告 + 可读的 400，而不是把园区编辑打成 500。
    let allowed = match auth::allowed_device_codes(state, claims).await {
        Ok(allowed) => allowed,
        Err(error) => {
            tracing::warn!("校验设备 {device_code} 的分配关系失败，按无权处理: {error}");
            return Err(denied());
        }
    };
    match allowed {
        Some(codes) if codes.contains(device_code) => Ok(()),
        _ => Err(denied()),
    }
}
