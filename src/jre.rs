//! JRE 自动下载（Adoptium Temurin）：loader 安装器（Forge/NeoForge 处理器）
//! 与模组化游戏运行都需要 Java——只用于安装/启动模组版，vanilla 原生启动不碰它。
//! JRE 落在项目 data\jre\<major>\ 内，不动系统目录。

use std::path::{Path, PathBuf};

const ADOPTUM_API: &str = "https://api.adoptium.net/v3/assets/latest";

/// 确保指定大版本的 JRE 可用，返回 java.exe 路径。
pub async fn ensure(client: &reqwest::Client, data: &Path, major: u32) -> Result<PathBuf, String> {
    let root = data.join("jre").join(major.to_string());
    let java = find_java_exe(&root);
    if let Some(p) = java {
        return Ok(p);
    }
    println!("下载 Temurin JRE {major}（Adoptium）……");
    let url = format!(
        "{ADOPTUM_API}/{major}/hotspot?architecture=x64&image_type=jre&os=windows&vendor=eclipse"
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Adoptium API：{e}"))?;
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("{e}"))?;
    let link = v
        .get(0)
        .and_then(|x| x.pointer("/binary/package/link"))
        .and_then(|x| x.as_str())
        .ok_or("Adoptium 响应缺少 package link")?
        .to_string();
    let name = v
        .get(0)
        .and_then(|x| x.pointer("/binary/package/name"))
        .and_then(|x| x.as_str())
        .unwrap_or("jre.zip")
        .to_string();

    let zip_path = data.join("jre").join(&name);
    let (f, _, _) = crate::fetch::download_batch(
        client,
        vec![(link, zip_path.clone(), String::new(), 0)],
        &format!("jre-{major}"),
    )
    .await?;
    let _ = f;
    println!("解压 JRE……");
    let fp = std::fs::File::open(&zip_path).map_err(|e| format!("{e}"))?;
    let mut z = zip::ZipArchive::new(fp).map_err(|e| format!("jre zip：{e}"))?;
    // 逐条目解压：个别条目失败不致命（只要 java.exe 在即可）
    let mut failed = 0usize;
    for i in 0..z.len() {
        let mut entry = match z.by_index(i) {
            Ok(e) => e,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        let Some(rel) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            failed += 1;
            continue;
        };
        let out = root.join(rel);
        if entry.is_dir() {
            let _ = std::fs::create_dir_all(&out);
            continue;
        }
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut w = match std::fs::File::create(&out) {
            Ok(w) => w,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        if std::io::copy(&mut entry, &mut w).is_err() {
            failed += 1;
        }
    }
    let _ = std::fs::remove_file(&zip_path);
    if failed > 0 {
        println!("解压：{failed} 个条目失败（非致命）");
    }
    find_java_exe(&root).ok_or_else(|| format!("JRE {major} 解压后没找到 java.exe"))
}

/// root 或其一级子目录里找 bin\java.exe。
fn find_java_exe(root: &Path) -> Option<PathBuf> {
    let direct = root.join("bin").join("java.exe");
    if direct.exists() {
        return Some(direct);
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path().join("bin").join("java.exe");
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}
