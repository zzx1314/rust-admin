use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::map::domain::{
    CreateMapRequest, MapEditRequest, MapForbiddenRequest, MapImageVO, MapPageQuery,
    MapShareResult, MapShareVO, MapVO, PushMapRequest, ShareMapRequest, UpdateMapRequest,
    UploadMapRequest,
};
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use axum::{
    Extension, Json,
    extract::{Multipart, Path, Query, State},
};

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

pub async fn create_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateMapRequest>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.create(req, &scope).await?;
    let vo = state.dog_map_service.to_vo(map).await?;
    Ok(Json(ApiResponse::ok(vo)))
}

/// `POST /api/dogMap/upload` — stores a `.zip` map package.
///
/// Multipart parts: `file` (the package) and `mapInfo`
/// (`{ "mapName": "园区地图", "remarks": "...", "id": 1 }`, `id` updates an
/// existing record instead of creating a new one).
pub async fn upload_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let mut info: Option<UploadMapRequest> = None;
    let mut file: Option<(String, Vec<u8>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::BadRequest(format!("无效的上传请求: {error}")))?
    {
        match field.name() {
            Some("file") => {
                let name = field.file_name().unwrap_or("map.zip").to_string();
                let data = field
                    .bytes()
                    .await
                    .map_err(|error| AppError::BadRequest(format!("读取上传文件失败: {error}")))?;
                file = Some((name, data.to_vec()));
            }
            Some("mapInfo") => {
                let text = field
                    .text()
                    .await
                    .map_err(|error| AppError::BadRequest(format!("读取地图信息失败: {error}")))?;
                info =
                    Some(serde_json::from_str(&text).map_err(|error| {
                        AppError::BadRequest(format!("地图信息格式错误: {error}"))
                    })?);
            }
            _ => {}
        }
    }

    let info = info.ok_or_else(|| AppError::BadRequest("缺少 mapInfo".to_string()))?;
    let (file_name, bytes) =
        file.ok_or_else(|| AppError::BadRequest("请上传地图包文件".to_string()))?;

    let scope = user.data_scope();
    let map = state
        .dog_map_service
        .save_upload(info, &file_name, &bytes, &state.upload_dir, &scope)
        .await?;
    let vo = state.dog_map_service.to_vo(map).await?;

    Ok(Json(ApiResponse::ok(vo)))
}

pub async fn get_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.get_by_id(params.id, &scope).await?;
    let vo = state.dog_map_service.to_vo(map).await?;
    Ok(Json(ApiResponse::ok(vo)))
}

pub async fn list_maps_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(query): Query<MapPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<MapVO>>>, AppError> {
    let result = state
        .dog_map_service
        .list_page(query, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_maps_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
) -> Result<Json<ApiResponse<Vec<MapVO>>>, AppError> {
    let maps = state.dog_map_service.list_all(&user.data_scope()).await?;
    Ok(Json(ApiResponse::ok(maps)))
}

pub async fn update_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateMapRequest>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.update(params.id, req, &scope).await?;
    let vo = state.dog_map_service.to_vo(map).await?;
    Ok(Json(ApiResponse::ok(vo)))
}

/// `GET /api/dogMap/{id}/image` — 把地图包里的栅格 PGM 转成 PNG 返回，
/// 优先 `map_edit/map.pgm`（遥控器编辑版），没有时才用原始 `model.pgm` / `map.pgm`；
/// 同时带回 `resolution` / `origin`，供路线规划页做像素↔世界坐标换算。
pub async fn get_map_image_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapImageVO>>, AppError> {
    let image = state
        .dog_map_service
        .load_image(params.id, &state.upload_dir, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(image)))
}

/// `POST /api/dogMap/{id}/edit` — 橡皮擦：按笔画改写编辑版的栅格噪点。
///
/// 包内有 `map_edit/` 时改写 `map_edit/map.pgm`（遥控器编辑后的地图），根目录
/// 的原始 `map.pgm` 不动；没有编辑目录时才回退到 `model.pgm` / `map.pgm`。
/// 请求体：`{ strokes: [{ points: [{x,y}], radius, value }], eraseValue: 254 }`，
/// `value` 是这一笔自己的灰度值（254 可通行 / 128 未知，旧的 205 归一到 128；
/// 缺省回退 `eraseValue`），坐标是栅格像素坐标；返回改写后的 PNG 与新的 `mapHash`。
pub async fn edit_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<MapEditRequest>,
) -> Result<Json<ApiResponse<MapImageVO>>, AppError> {
    let image = state
        .dog_map_service
        .erase(params.id, req, &state.upload_dir, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(image)))
}

/// `POST /api/dogMap/{id}/forbidden` — 设置禁止区域（**整表替换**）。
///
/// 请求体：`{ areas: [{ id?, center: {x,y}, width, height, rotation }] }`，
/// 坐标与边长都是栅格像素（左上角原点）。新增/保留的区域在编辑版栅格上刷成 `0`
/// （占据，与遥控器一致），删掉的按建图原图还原；`edit_info.json` 的
/// `forbidden_areas` / `edit_ops` 同步换掉。返回新 PNG 与当前禁止区域列表。
pub async fn set_forbidden_areas_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<MapForbiddenRequest>,
) -> Result<Json<ApiResponse<MapImageVO>>, AppError> {
    let image = state
        .dog_map_service
        .set_forbidden_areas(params.id, req, &state.upload_dir, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(image)))
}

/// `POST /api/dogMap/{id}/restore` — 恢复原图。
///
/// 把 `map_edit/map.pgm` 换回根目录的建图原图，并把遥控器的编辑记录
/// （`map_edit/edit_info.json`）清空——不清空的话它下次保存会按「根图 + 旧擦除圆」
/// 把擦除又画回来。只改平台侧的包，写回本体仍需「下发到狗体」。
pub async fn restore_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapImageVO>>, AppError> {
    let image = state
        .dog_map_service
        .restore_original(params.id, &state.upload_dir, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(image)))
}

/// `POST /api/dogMap/{id}/push` — 把中台编辑后的地图下发到狗体（本体）。
///
/// 链路：中台 → 业务平台（反向控制信道 / 直连 HTTP）→ 设备客户端 → 本体文件服务，
/// 由业务平台把最新包写进本体地图目录并重新载图。中台只做授权与审计。
pub async fn push_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    req: Option<Json<PushMapRequest>>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let req = req.map(|Json(req)| req).unwrap_or_default();
    let map = state
        .dog_map_service
        .get_by_id(params.id, &user.data_scope())
        .await?;

    // 可选目标：上报设备在前，分享设备在后（分享不复制地图，只是同一张图的另一台使用者）
    let mut candidates: Vec<String> = Vec::new();
    if let Some(code) = map
        .device_code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
    {
        candidates.push(code.to_string());
    }
    for code in state.dog_map_service.share_codes(map.id).await? {
        if !candidates.iter().any(|known| known == &code) {
            candidates.push(code);
        }
    }
    let device_code = match req
        .device_code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
    {
        Some(code) => {
            if !candidates.iter().any(|known| known == code) {
                return Err(AppError::BadRequest(format!(
                    "地图「{}」没有关联也没有分享给设备 {}；请先在地图列表里把这张图分享给该设备",
                    map.map_name, code
                )));
            }
            code.to_string()
        }
        None => candidates.first().cloned().ok_or_else(|| {
            AppError::BadRequest(
                "该地图既没有关联上报设备，也没有分享给其他设备，无法下发；请先分享给目标设备"
                    .to_string(),
            )
        })?,
    };
    if map
        .file_path
        .as_deref()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return Err(AppError::BadRequest(
            "该地图尚未上传地图包，无法下发到狗体".to_string(),
        ));
    }
    // 授权：普通用户只能下发到自己被分配的设备
    state
        .dog_device_service
        .get_by_code(&device_code, &user.data_scope())
        .await?
        .ok_or_else(|| AppError::NotFound(format!("设备 {device_code} 不存在或未分配给你")))?;

    let request_id = crate::business::dog::device::handlers::resolve_request_id(req.request_id);
    let body = serde_json::json!({
        "mapId": map.id,
        "mapName": map.map_name,
        "mapVersion": map.map_version,
        "mapHash": map.map_hash,
        "deviceCode": device_code,
        "requestId": request_id,
    });
    let data = crate::business::dog::device::handlers::forward(
        &state,
        &user,
        &device_code,
        "下发地图",
        "push-map",
        &request_id,
        body,
    )
    .await?;
    Ok(Json(ApiResponse::ok(data)))
}

pub async fn delete_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .dog_map_service
        .delete(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

/// `POST /api/dogMap/{id}/publish` — 发布为园区当前生效基线，旧版本自动归档。
pub async fn publish_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.publish(params.id, &scope).await?;
    let vo = state.dog_map_service.to_vo(map).await?;
    Ok(Json(ApiResponse::ok(vo)))
}

/// `POST /api/dogMap/{id}/newVersion` — 派生下一个版本（**草稿**，可编辑 / 覆盖包），
/// 确认后再调 `publish`。
pub async fn new_map_version_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.new_version(params.id, &scope).await?;
    let vo = state.dog_map_service.to_vo(map).await?;
    Ok(Json(ApiResponse::ok(vo)))
}

/// `GET /api/dogMap/{id}/shares` — 这张地图分享给了哪些设备。
pub async fn list_map_shares_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<Vec<MapShareVO>>>, AppError> {
    let shares = state
        .dog_map_service
        .list_shares(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(shares)))
}

/// `POST /api/dogMap/{id}/share` — 把这张地图分享给其他设备使用，不用重新建图。
///
/// 请求体：`{ "deviceCodes": ["RD-002", "RD-003"], "mode": "replace" }`。
/// `mode = append`（默认）只追加；`replace` 以本次提交为准（空数组 = 清空分享）。
pub async fn share_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<ShareMapRequest>,
) -> Result<Json<ApiResponse<MapShareResult>>, AppError> {
    let scope = user.data_scope();
    // 设备必须在中台台账里，且（非管理员）在分配给自己的设备范围内
    let mut device_codes = Vec::new();
    for code in &req.device_codes {
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        state
            .dog_device_service
            .get_by_code(code, &scope)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("设备 {code} 不存在或未分配给你")))?;
        device_codes.push(code.to_string());
    }
    let result = state
        .dog_map_service
        .share(
            params.id,
            ShareMapRequest {
                device_codes,
                mode: req.mode,
            },
            &scope,
        )
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

/// `DELETE /api/dogMap/{id}/share/{device_code}` — 取消分享给单台设备。
pub async fn unshare_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path((id, device_code)): Path<(i64, String)>,
) -> Result<Json<ApiResponse<MapShareResult>>, AppError> {
    let result = state
        .dog_map_service
        .unshare(id, &device_code, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}
