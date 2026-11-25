//! 启动器设置与路径策略。
//!
//! - 引导配置：exe 同目录 `mc-native-exp.config.json`（可删、可手改），记录
//!   数据目录指针（可迁移到任意磁盘）、版本隔离开关、默认游戏目录。
//! - 版本级覆盖：`<data>/versions/<id>/instance.json` 的 `game_dir` 字段，
//!   让单个版本用独立游戏目录（存档/模组/资源包互不影响）。
//!
//! 解析顺序（game dir）：版本级覆盖 → 隔离模式 `<data>/instances/<id>/game`
//! → 默认游戏目录（设置项）→ `<data>/game`。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_NAME: &str = "mc-native-exp.config.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// 数据目录（绝对路径）；空/缺失 = exe 同目录 data\
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    /// 多版本隔离：true 时每个版本用 `<data>\instances\<id>\game`
    #[serde(default)]
    pub isolation: bool,
    /// 默认游戏目录（隔离关闭时生效）；空 = `<data>\game`
    #[serde(default)]
    pub game_dir: Option<PathBuf>,
}

/// 引导配置路径：exe 同目录（便携语义，移动整个文件夹即整体迁移）。
pub fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(CONFIG_NAME)
}

pub fn load() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let path = config_path();
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| e.to_string())?;
    crate::manifest::write_atomic(&path, &bytes)
}

/// 默认数据目录：exe 同目录 data\。
pub fn default_data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("data")
}

/// 解析数据目录：显式 --data 优先，其次引导配置，最后默认。
pub fn resolve_data_dir(cli_data: Option<&Path>) -> PathBuf {
    if let Some(p) = cli_data {
        return if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(p)
        };
    }
    if let Some(p) = load().data_dir {
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    default_data_dir()
}

/* ── 版本级实例配置 ───────────────────────────────────────── */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstanceConfig {
    /// 该版本专属游戏目录；空 = 跟随全局策略
    #[serde(default)]
    pub game_dir: Option<PathBuf>,
}

pub fn instance_path(data: &Path, id: &str) -> PathBuf {
    data.join("versions").join(id).join("instance.json")
}

pub fn load_instance(data: &Path, id: &str) -> InstanceConfig {
    std::fs::read_to_string(instance_path(data, id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_instance(data: &Path, id: &str, cfg: &InstanceConfig) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| e.to_string())?;
    crate::manifest::write_atomic(&instance_path(data, id), &bytes)
}

/* ── 游戏目录解析 ─────────────────────────────────────────── */

/// 解析某版本应使用的游戏目录。
pub fn resolve_game_dir(data: &Path, id: &str) -> PathBuf {
    // 1) 版本级覆盖
    if let Some(p) = load_instance(data, id).game_dir {
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let cfg = load();
    // 2) 隔离模式：每版本独立目录
    if cfg.isolation {
        return data.join("instances").join(safe_dir_name(id)).join("game");
    }
    // 3) 默认游戏目录设置
    if let Some(p) = cfg.game_dir {
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    // 4) 兜底：共享 data\game
    data.join("game")
}

/// 版本 id → 安全目录名（防路径穿越/非法字符）。
pub fn safe_dir_name(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_strip_path_chars() {
        assert_eq!(safe_dir_name("fabric-loader-0.20.1-1.21.4"), "fabric-loader-0.20.1-1.21.4");
        assert_eq!(safe_dir_name("../evil"), "___evil");
        assert_eq!(safe_dir_name("a:b*c?"), "a_b_c_");
    }
}
