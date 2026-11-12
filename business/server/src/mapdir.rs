//! 本体地图目录名：`<map_dir_prefix>/history_map/<目录名>` 的解析与补齐。
//!
//! 智航只认**本体上的目录名**（4013 `map_path_list[].path` 的最后一段，目录名由
//! 建图会话决定，如 `2026_11_10_16_15_06`）。业务侧的 `map_versions.id`
//! （`ROS-MAP-1794446794789@v1.0`）是平台自造的，直接 `@`→`_` 得到的是一个
//! 本体上不存在的目录：5000/4030 会指到空目录，本体只回一句
//! `13898 map not found or invalid (missing map.yaml)`，很难从字面上看出是「目录名对不上」。
//!
//! 本体目录名的唯一可靠来源是 `map_versions.source_ref`
//! （设备回传地图时写入 `/ota/alg_data/map/history_map/<目录名>`）。
//! 本模块负责：
//!
//! 1. 从 `source_ref` / 包名里取目录名；
//! 2. 把历史导入缺 `source_ref` 的行按「同一个 zip 包」补齐；
//! 3. 下发时优先给出真实目录名，缺失才退回业务 ID 的合成名
//!    （客户端仍会用 4013 列表核对，对不上就拒绝下发）。
//!
//! 注意：这里只解决「目录名」，不解决「文件缺失」——本体目录里必须是
//! `map.yaml` / `map.pgm` / `map.geojson` / `map.pcd` 这一套智航命名，
//! ROS map_server 的 `model.*` 包即使目录名对了也载不进去。

use sqlx::SqlitePool;

/// 从本体地图路径里取目录名：`/ota/.../history_map/2026_11_10_16_15_06` → `2026_11_10_16_15_06`
pub fn dir_from_source_ref(source_ref: &str) -> Option<String> {
    let trimmed = source_ref.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed).trim();
    (!last.is_empty()).then(|| last.to_string())
}

/// 地图包文件名 → 目录名（去掉 `.zip` 与目录前缀）
///
/// 智航导出的包一般就叫 `<目录名>.zip`；ROS map_server 的 `model.zip` 也会得到
/// `model`，所以这只是「候选」，还要和服务端已知的目录名比对才算匹配。
pub fn dir_from_file_name(file_name: &str) -> Option<String> {
    let trimmed = file_name.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let stem = base
        .strip_suffix(".zip")
        .or_else(|| base.strip_suffix(".ZIP"))
        .unwrap_or(base)
        .trim();
    (!stem.is_empty() && stem != "model").then(|| stem.to_string())
}

/// 业务地图 ID → 目录名的合成值（本体侧目录名未知时的兜底）
pub fn synthetic_dir(map_id: &str) -> String {
    map_id
        .chars()
        .map(|c| match c {
            '@' | '/' | '\\' | ' ' => '_',
            other => other,
        })
        .collect()
}

/// 按包名在同一张表里找已登记的本体目录（`source_ref`）。
///
/// 用于 `import-backend`：从中台重新导入一个设备已经回传过的包时，把本体目录名
/// 一并认回来，避免生成一个「只有业务 ID、本体上找不到」的重复地图。
pub async fn source_ref_for_file_name(
    pool: &SqlitePool,
    file_name: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let Some(stem) = file_name.and_then(dir_from_file_name) else {
        return Ok(None);
    };
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT source_ref FROM map_versions
         WHERE source_ref IS NOT NULL AND source_ref <> ''",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .find(|(source_ref,)| {
            dir_from_source_ref(source_ref).as_deref() == Some(stem.as_str())
        })
        .map(|(source_ref,)| source_ref.trim().to_string()))
}

/// 某张地图版本的目录名：优先 `source_ref`，否则退回合成名
pub async fn map_dir(pool: &SqlitePool, map_id: &str) -> Result<String, sqlx::Error> {
    let source_ref: Option<Option<String>> =
        sqlx::query_scalar("SELECT source_ref FROM map_versions WHERE id = ?1")
            .bind(map_id)
            .fetch_optional(pool)
            .await?;
    Ok(source_ref
        .flatten()
        .as_deref()
        .and_then(dir_from_source_ref)
        .unwrap_or_else(|| synthetic_dir(map_id)))
}

/// 按「同一个 zip 包」把缺 `source_ref` 的行补齐。
///
/// 规则：包名去掉 `.zip` 后，若**恰好**匹配到另一行 `source_ref` 的目录名，就复用
/// 那条 `source_ref`。典型场景：设备回传过 `2026_11_10_16_15_06.zip`（记了
/// `source_ref`），运营又从中台 `dog_map` 导入了同一个包（只有 `file_name`），
/// 两者应当指向本体上的同一个目录。
///
/// 幂等：已有 `source_ref` 的行不动，可以随进程启动反复执行。
pub async fn backfill_source_refs(pool: &SqlitePool) -> Result<usize, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, source_ref, file_name FROM map_versions")
            .fetch_all(pool)
            .await?;

    // 目录名 → source_ref（同名目录以先入库的为准）
    let mut known: Vec<(String, String)> = Vec::new();
    for (_, source_ref, _) in &rows {
        if let Some(source_ref) = source_ref.as_deref()
            && let Some(dir) = dir_from_source_ref(source_ref)
            && !known.iter().any(|(name, _)| name == &dir)
        {
            known.push((dir, source_ref.trim().to_string()));
        }
    }

    let mut filled = 0_usize;
    for (id, source_ref, file_name) in &rows {
        if source_ref.as_deref().is_some_and(|v| !v.trim().is_empty()) {
            continue;
        }
        let Some(stem) = file_name.as_deref().and_then(dir_from_file_name) else {
            continue;
        };
        let matches: Vec<&(String, String)> =
            known.iter().filter(|(dir, _)| dir == &stem).collect();
        let [matched] = matches.as_slice() else {
            continue;
        };
        sqlx::query("UPDATE map_versions SET source_ref = ?1 WHERE id = ?2")
            .bind(&matched.1)
            .bind(id)
            .execute(pool)
            .await?;
        filled += 1;
        tracing::info!(
            map = %id,
            source_ref = %matched.1,
            "地图本体目录名已按同名包回填"
        );
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_from_source_ref_takes_last_segment() {
        assert_eq!(
            dir_from_source_ref("/ota/alg_data/map/history_map/2026_11_10_16_15_06/").as_deref(),
            Some("2026_11_10_16_15_06")
        );
        assert_eq!(
            dir_from_source_ref("2026_11_10_16_15_06").as_deref(),
            Some("2026_11_10_16_15_06")
        );
        assert_eq!(dir_from_source_ref("  "), None);
    }

    #[test]
    fn dir_from_file_name_strips_zip_and_skips_model() {
        assert_eq!(
            dir_from_file_name("2026_11_10_16_15_06.zip").as_deref(),
            Some("2026_11_10_16_15_06")
        );
        assert_eq!(dir_from_file_name("maps/a/model.zip"), None);
        assert_eq!(dir_from_file_name(""), None);
    }

    #[test]
    fn synthetic_dir_replaces_unsafe_chars() {
        assert_eq!(synthetic_dir("ROS-MAP-3@v1.0"), "ROS-MAP-3_v1.0");
        assert_eq!(synthetic_dir("A/B C"), "A_B_C");
    }
}
