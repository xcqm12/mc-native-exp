//! 小七伙伴模组（xiaoqi-companion）的随包分发与安装。
//!
//! 两个 jar 以 `include_bytes!` 嵌进启动器（单文件分发，CI 无需 JDK/Gradle）：
//! - `xiaoqi-companion.jar`：模组本体（实体 + 渲染 + 客户端逻辑）
//! - `fabric-api.jar`：Fabric API 0.119.4+1.21.4（模组依赖；实例已有任意
//!   fabric-api 时不重复安装，避免重复模块）
//!
//! 启动 Fabric/Quilt 1.21.4 实例前写入 `<game_dir>/mods/`，内容变化才覆盖。

use std::path::{Path, PathBuf};

pub const MOD_BYTES: &[u8] = include_bytes!("../resources/xiaoqi-companion.jar");
pub const API_BYTES: &[u8] = include_bytes!("../resources/fabric-api.jar");
pub const MOD_FILE: &str = "xiaoqi-companion.jar";
pub const API_FILE: &str = "fabric-api-0.119.4+1.21.4.jar";

/// 支持小七的实例：Fabric/Quilt 且 MC 版本为 1.21.4（模组编译目标）。
pub fn supports(id: &str, base_mc: &str) -> bool {
    let loader_ok = id.contains("fabric") || id.contains("quilt");
    loader_ok && base_mc.starts_with("1.21.4")
}

/// 写入游戏目录的 mods/，返回模组 jar 路径；内容相同则不动（秒启动）。
pub fn ensure_installed(game_dir: &Path) -> Result<PathBuf, String> {
    let mods = game_dir.join("mods");
    std::fs::create_dir_all(&mods).map_err(|e| format!("创建 mods 目录失败：{e}"))?;

    // 已有任意 fabric-api（用户自己装的）就不动，避免重复模块
    let has_api = std::fs::read_dir(&mods)
        .map(|it| {
            it.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                n.starts_with("fabric-api") && n.ends_with(".jar")
            })
        })
        .unwrap_or(false);
    if !has_api {
        write_if_changed(&mods.join(API_FILE), API_BYTES)?;
    }

    let target = mods.join(MOD_FILE);
    write_if_changed(&target, MOD_BYTES)?;
    Ok(target)
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let needs_write = match std::fs::read(path) {
        Ok(old) => old != bytes,
        Err(_) => true,
    };
    if needs_write {
        crate::manifest::write_atomic(path, bytes)?;
        println!("已安装：{}", path.display());
    }
    Ok(())
}
