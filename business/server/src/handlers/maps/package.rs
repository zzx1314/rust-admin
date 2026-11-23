//! 地图包原文的落地路径与下载（设备客户端「下发地图」拉这里）。

// 同一模块拆出来的几块共用一个词汇表（`MAP_SELECT` / helper / `AppState`），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/// 地图包文件的绝对路径。
///
/// 业务库 `map_versions.file_path` 有两种形态：
/// - 从 `backend` 导入：相对路径（`maps/xxx.zip`），落在 `<backend_db>/device_uploads/`；
/// - 设备回传 / 下发刷新：绝对路径（本服务 `ingest_dir` 下的暂存包）。
pub(crate) fn resolve_package_path(
    backend_db: &std::path::Path,
    file_path: &str,
) -> Option<std::path::PathBuf> {
    let trimmed = file_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = std::path::Path::new(trimmed);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    let upload_root = backend_db.parent()?.join("device_uploads");
    let candidate = upload_root.join(path);
    candidate.is_file().then_some(candidate)
}

/// `GET /api/map-versions/{id}/package` — 原样返回地图包（zip）。
///
/// 设备客户端「下发地图」用它拉最新包；中台编辑后业务库会先被刷新
/// （见 [`apply_push_map`]），所以这里拿到的永远是最新内容。
pub async fn map_package(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT file_path, file_name FROM map_versions WHERE id = ?1")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let Some((file_path, file_name)) = row else {
        return Err(AppError::NotFound(format!("地图基线 {id} 不存在")));
    };
    let file_path = file_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("该地图没有关联的地图包".to_string()))?;
    let path = resolve_package_path(&state.config.backend_db_path, file_path).ok_or_else(|| {
        AppError::NotFound(format!(
            "地图包文件不存在（{file_path}），请重新从设备同步或从中台导入"
        ))
    })?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| AppError::Internal(format!("读取地图包失败: {error}")))?;
    let download_name = file_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "map.zip".to_string());

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{download_name}\""),
        )
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|error| AppError::Internal(format!("构造地图包响应失败: {error}")))
}
