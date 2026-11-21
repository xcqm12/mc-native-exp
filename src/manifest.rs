//! 版本清单：piston-meta version_manifest_v2.json，含全部历史版本。

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Deserialize)]
struct RawManifest {
    latest: Latest,
    versions: Vec<VersionEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub time: String,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
    #[serde(default)]
    pub sha1: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub latest: Latest,
    pub versions: Vec<VersionEntry>,
    pub fetched_at: u64,
}

pub fn manifest_path(data: &Path) -> std::path::PathBuf {
    data.join("manifest.json")
}

/// 读已缓存的清单。
pub fn load(data: &Path) -> Result<Manifest, String> {
    let path = manifest_path(data);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| "清单不存在——先运行 `manifest` 子命令拉取".to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("清单损坏：{e}"))
}

/// 拉取并缓存清单（含全部版本）。
pub async fn fetch(client: &reqwest::Client, data: &Path) -> Result<(), String> {
    let resp = client
        .get(MANIFEST_URL)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("拉取清单失败：{e}"))?;
    let raw: RawManifest = resp.json().await.map_err(|e| format!("解析清单失败：{e}"))?;
    let m = Manifest {
        latest: raw.latest,
        versions: raw.versions,
        fetched_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let path = manifest_path(data);
    let bytes = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
    write_atomic(&path, &bytes)?;
    let releases = m.versions.iter().filter(|v| v.kind == "release").count();
    println!(
        "清单已保存：{} 个版本（正式版 {}，快照 {}），最新正式版 {}",
        m.versions.len(),
        releases,
        m.versions.len() - releases,
        m.latest.release
    );
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("写临时文件失败：{e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("原子改名失败：{e}"))
}

/// 找版本条目：精确 id；省略时取最新正式版。
pub fn resolve<'m>(m: &'m Manifest, version: Option<&str>) -> Result<&'m VersionEntry, String> {
    match version {
        Some(id) => m.versions.iter().find(|v| v.id == id).ok_or_else(|| {
            let suggest = m
                .versions
                .iter()
                .filter(|v| v.id.starts_with(id))
                .take(5)
                .map(|v| v.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if suggest.is_empty() {
                format!("清单里没有版本 {id}")
            } else {
                format!("清单里没有版本 {id}，相近的：{suggest}")
            }
        }),
        None => Ok(m
            .versions
            .iter()
            .find(|v| v.id == m.latest.release)
            .ok_or_else(|| format!("清单里找不到最新正式版 {}", m.latest.release))?),
    }
}
