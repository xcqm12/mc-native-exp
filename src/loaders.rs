//! 模组加载器自动安装：Fabric / Quilt（meta API 直装，无需 Java）+
//! Forge / NeoForge（官方安装器 + 自动 JRE 跑处理器）。
//!
//! 安装全部写进项目 data\ 内（versions / libraries / game），不碰系统与用户目录。
//! OptiFine / LiteLoader 属补丁型 / 停更加载器，不在本实验自动化范围（README 有说明）。

use crate::fetch;
use crate::jre;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const SUPPORTED: &[&str] = &["fabric", "quilt", "neoforge", "forge"];

/// 安装入口：loader + MC 版本（loader 版本省略 = 该 MC 的最新支持版）。
pub async fn install(
    client: &reqwest::Client,
    data: &Path,
    loader: &str,
    mc: &str,
    loader_version: Option<&str>,
) -> Result<String, String> {
    // 正版前提：安装/取用前必须持有有效会话
    crate::auth::ensure_session(client, data).await?;
    match loader {
        "fabric" | "quilt" | "neoforge" | "forge" => {
            install_via_installer(client, data, loader, mc, loader_version).await
        }
        other => Err(format!(
            "不支持的加载器 {other}——实验支持：{}（OptiFine/LiteLoader 不在自动安装范围）",
            SUPPORTED.join("/")
        )),
    }
}

/* ── Fabric / Quilt：meta API 直装 ─────────────────────────── */

fn meta_base(loader: &str) -> &'static str {
    match loader {
        "fabric" => "https://meta.fabricmc.net/v2/versions/loader",
        _ => "https://meta.quiltmc.org/v3/versions/loader",
    }
}

/// Fabric/Quilt 安装器（jar）最新版（maven-metadata 的 <release>）——不是 loader 版本。
pub async fn latest_installer_version(client: &reqwest::Client, loader: &str) -> Result<String, String> {
    let url = match loader {
        "fabric" => "https://maven.fabricmc.net/net/fabricmc/fabric-installer/maven-metadata.xml",
        _ => "https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-installer/maven-metadata.xml",
    };
    let resp = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("{loader} maven：{e}"))?;
    let text = resp.text().await.map_err(|e| format!("{e}"))?;
    regex_like_release(&text).ok_or_else(|| format!("{loader} metadata 无 release"))
}

/// Fabric/Quilt loader 最新版（meta 列表第一个 stable）——这才是 -loader 参数。
pub async fn latest_loader_version(
    client: &reqwest::Client,
    loader: &str,
    mc: &str,
) -> Result<String, String> {
    let url = match loader {
        "fabric" => format!("https://meta.fabricmc.net/v2/versions/loader/{mc}"),
        _ => format!("https://meta.quiltmc.org/v3/versions/loader/{mc}"),
    };
    let arr: Value = client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("{loader} meta：{e}"))?
        .json()
        .await
        .map_err(|e| format!("{e}"))?;
    let items = arr
        .as_array()
        .ok_or_else(|| format!("{loader} 还没有支持 {mc} 的版本"))?;
    for it in items {
        let v = it.pointer("/loader/version").and_then(|x| x.as_str());
        let stable = it
            .pointer("/loader/stable")
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        if let Some(v) = v {
            if stable {
                return Ok(v.to_string());
            }
        }
    }
    items
        .first()
        .and_then(|it| it.pointer("/loader/version").and_then(|x| x.as_str()))
        .map(String::from)
        .ok_or_else(|| format!("{loader} meta 无版本"))
}

fn regex_like_release(text: &str) -> Option<String> {
    let start = text.find("<release>")? + "<release>".len();
    let end = text[start..].find("</release>")? + start;
    Some(text[start..end].to_string())
}

/// 统一安装器路线：下载 loader 官方安装器 jar → 自动 JRE 执行 client 安装 →
/// 找生成的版本 JSON → 下载全套文件。
/// - fabric/quilt：`client -dir <game> -mcversion <mc> -loader <lv>`
/// - forge/neoforge：`--installClient <game>`（安装器内部跑处理器打补丁）
async fn install_via_installer(
    client: &reqwest::Client,
    data: &Path,
    loader: &str,
    mc: &str,
    loader_version: Option<&str>,
) -> Result<String, String> {
    // loader 版本（-loader 参数）与安装器版本（jar）是两个东西
    let lv = match loader_version {
        Some(v) => v.to_string(),
        None => {
            match loader {
                "fabric" | "quilt" => latest_loader_version(client, loader, mc).await?,
                "neoforge" => latest_neoforge(client, mc).await?,
                _ => latest_forge(client, mc).await?,
            }
        }
    };
    let installer_ver = if matches!(loader, "fabric" | "quilt") {
        Some(latest_installer_version(client, loader).await?)
    } else {
        None
    };
    println!("{loader} loader {lv} / installer {installer_ver:?}（MC {mc}）");

    // 原版 JSON 先就位（安装器要 inheritsFrom 的父版本与父 jar）
    fetch::fetch_version(client, data, Some(mc), true).await?;

    // 安装器 jar
    let url = installer_url(client, loader, mc, installer_ver.as_deref().unwrap_or(&lv)).await?;
    let dl_dir = data.join("downloads");
    std::fs::create_dir_all(&dl_dir).map_err(|e| format!("{e}"))?;
    let installer = dl_dir.join(format!("{loader}-{lv}-installer.jar"));
    download_file(client, &url, &installer, &format!("{loader} installer")).await?;

    // forge/neoforge：预下载安装器 libraries（其内部下载器无重试，绕开）
    if matches!(loader, "forge" | "neoforge") {
        predownload_installer_libs(client, data, &installer).await?;
    }

    // JRE（安装器与模组版游戏都需要）
    let java_major = required_java_major(data, mc).unwrap_or(21);
    let java = jre::ensure(client, data, java_major).await?;
    println!("用 JRE：{}", java.display());

    // 安装器要求 -dir 下有 versions/（原版 JSON）与 launcher_profiles.json；
    // data/ 本身就是这个布局（data/versions、data/libraries）。
    let profiles = data.join("launcher_profiles.json");
    if !profiles.exists() {
        std::fs::write(&profiles, br#"{"profiles":{},"settings":{},"version":3}"#)
            .map_err(|e| format!("{e}"))?;
    }
    let mut cmd = tokio::process::Command::new(&java);
    cmd.arg("-jar").arg(&installer);
    match loader {
        "fabric" | "quilt" => {
            cmd.args(["client", "-dir"]).arg(data)
                .args(["-mcversion", mc]).args(["-loader", &lv]);
        }
        _ => {
            cmd.arg("--installClient").arg(data);
        }
    }
    cmd.current_dir(data);
    let out = cmd.output().await.map_err(|e| format!("安装器启动失败：{e}"))?;
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log_path = data.join(format!("{loader}-install.log"));
    let _ = std::fs::write(&log_path, &log);
    if !out.status.success() {
        let tail: String = log.chars().rev().take(2000).collect::<String>().chars().rev().collect();
        return Err(format!("安装器退出码 {:?}，日志尾部：{}", out.status.code(), tail));
    }

    let id = find_installed(data, data, loader, &lv)?;
    println!("安装完成：{id}");
    crate::profile::fetch_profile(client, data, &id).await?;
    Ok(id)
}

/* ── Forge / NeoForge：官方安装器 + 自动 JRE ───────────────── */

/// NeoForge 最新版：多源回退（BMCLAPI metadata → 官方 metadata → 官方 API）。
/// 版本号形如 <mc>.<build>。
async fn latest_neoforge(client: &reqwest::Client, mc: &str) -> Result<String, String> {
    // 1) BMCLAPI 镜像 metadata
    let meta_urls = [
        "https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/maven-metadata.xml".to_string(),
        "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml".to_string(),
    ];
    for url in &meta_urls {
        for attempt in 0..2u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            if let Ok(resp) = client.get(url).send().await {
                if let Ok(text) = resp.text().await {
                    if let Some(rel) = regex_like_release(&text) {
                        if rel.starts_with(mc) {
                            return Ok(rel);
                        }
                    }
                    // metadata 全量版本列表里挑 <mc> 开头的最后一个
                    if let Some(v) = last_version_matching(&text, mc) {
                        return Ok(v);
                    }
                }
            }
            println!("NeoForge 版本源 {} 不可达，重试/换源……", url.split('/').nth(2).unwrap_or("?"));
        }
    }
    // 2) 官方 API
    let url = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
    let v: Value = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("NeoForge maven：{e}"))?
        .json()
        .await
        .map_err(|e| format!("{e}"))?;
    let mut versions: Vec<String> = v
        .get("versions")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .filter(|s| s.starts_with(mc))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    versions.sort();
    versions
        .last()
        .cloned()
        .ok_or_else(|| format!("NeoForge 没有 {mc} 的版本"))
}

/// 从 maven-metadata.xml 文本里挑以 mc 开头的最后一个 <version>。
fn last_version_matching(text: &str, mc: &str) -> Option<String> {
    let mut last = None;
    let mut rest = text;
    while let Some(pos) = rest.find("<version>") {
        let start = pos + "<version>".len();
        let end = rest[start..].find("</version>")? + start;
        let v = &rest[start..end];
        if v.starts_with(mc) {
            last = Some(v.to_string());
        }
        rest = &rest[end..];
    }
    last
}

/// Forge 最新版（promotions API：1.21.4 → "51.0.x"）。
async fn latest_forge(client: &reqwest::Client, mc: &str) -> Result<String, String> {
    // files.minecraftforge.net 偶发不可达：BMCLAPI 镜像同一文件
    let urls = [
        "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json".to_string(),
        "https://bmclapi2.bangbang93.com/net/minecraftforge/forge/promotions_slim.json".to_string(),
    ];
    let mut v: Value = Value::Null;
    for url in &urls {
        if let Ok(resp) = client.get(url).send().await {
            if let Ok(text) = resp.text().await {
                if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                    v = parsed;
                    break;
                }
            }
        }
        println!("Forge promotions {} 不可达，换源……", url.split('/').nth(2).unwrap_or("?"));
    }
    if v.is_null() {
        return Err("Forge promotions 全部源不可达".into());
    }
    let key = format!("{mc}-latest");
    v.pointer("/promos")
        .and_then(|p| p.get(&key))
        .and_then(|x| x.as_str())
        .map(String::from)
        .ok_or_else(|| format!("Forge promotions 没有 {mc}"))
}

/// Forge/NeoForge 安装器 jar URL（官方源）。
async fn installer_url(
    client: &reqwest::Client,
    loader: &str,
    mc: &str,
    lv: &str,
) -> Result<String, String> {
    let _ = (client, mc);
    if loader == "fabric" {
        return Ok(format!(
            "https://maven.fabricmc.net/net/fabricmc/fabric-installer/{lv}/fabric-installer-{lv}.jar"
        ));
    }
    if loader == "quilt" {
        return Ok(format!(
            "https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-installer/{lv}/quilt-installer-{lv}.jar"
        ));
    }
    if loader == "neoforge" {
        return Ok(format!(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/{lv}/neoforge-{lv}-installer.jar"
        ));
    }
    // Forge：installer 直链（maven.cloudflare / files.minecraftforge.net）
    Ok(format!(
        "https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{lv}/forge-{mc}-{lv}-installer.jar"
    ))
}

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    label: &str,
) -> Result<(), String> {
    let (f, _, _) = fetch::download_batch(
        client,
        vec![(url.to_string(), dest.to_path_buf(), String::new(), 0)],
        label,
    )
    .await?;
    let _ = f;
    Ok(())
}

/// Forge/NeoForge：下载安装器 → 预下载其 libraries → 自动 JRE 跑 --installClient。
fn find_installed(data: &Path, game_dir: &Path, loader: &str, lv: &str) -> Result<String, String> {
    // 官方安装器把版本 JSON 写进 <game_dir>/versions/<id>/
    let dirs = [game_dir.join("versions"), data.join("versions")];
    for base in dirs {
        if let Ok(rd) = std::fs::read_dir(&base) {
            let mut best: Option<(String, std::time::SystemTime)> = None;
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_string();
                if !name.contains(loader) && !name.contains("forge") && !name.contains("neoforge") {
                    continue;
                }
                if !p.join(format!("{name}.json")).exists() {
                    continue;
                }
                let mt = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if best.as_ref().map(|(_, t)| mt > *t).unwrap_or(true) {
                    best = Some((name, mt));
                }
            }
            if let Some((name, _)) = best {
                // 安装器写进 game/versions 的：搬到 data/versions 统一管理
                if !data.join("versions").join(&name).exists() {
                    let from = base.join(&name);
                    let to = data.join("versions").join(&name);
                    if std::fs::rename(&from, &to).is_err() {
                        let _ = copy_dir(&from, &to);
                    }
                }
                return Ok(name);
            }
        }
    }
    let _ = lv;
    Err("安装器完成但未找到生成的版本 JSON".into())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        let t = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &t)?;
        } else {
            std::fs::copy(&p, &t)?;
        }
    }
    Ok(())
}

/// 从安装器 jar 里读 install_profile.json + version.json，预下载全部 libraries。
async fn predownload_installer_libs(
    client: &reqwest::Client,
    data: &Path,
    installer: &Path,
) -> Result<(), String> {
    let fp = std::fs::File::open(installer).map_err(|e| format!("{e}"))?;
    let mut z = zip::ZipArchive::new(fp).map_err(|e| format!("installer zip：{e}"))?;
    let lib_root = data.join("libraries");
    let mut jobs: Vec<(String, PathBuf, String, u64)> = Vec::new();
    for entry_name in ["install_profile.json", "version.json"] {
        if z.by_name(entry_name).is_err() {
            continue;
        }
        let mut e = z.by_name(entry_name).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_slice(&buf).map_err(|e| format!("{entry_name}：{e}"))?;
        let mut collect = |node: &Value| {
            if let Some(libs) = node.get("libraries").and_then(|x| x.as_array()) {
                for lib in libs {
                    if !crate::fetch::rules_allow_windows(lib.get("rules")) {
                        continue;
                    }
                    let name = lib.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                    // forge/neoforge 自产 :client/:server 条目是安装器内嵌数据
                    // （非 maven artifact，无下载文件）——跳过，安装器处理时自行释放
                    let lp: Vec<&str> = name.split(':').collect();
                    if lp.first().copied() == Some("net.minecraftforge") || lp.first().copied() == Some("net.neoforged") {
                        if lp.len() >= 4 && matches!(lp[3], "client" | "server") {
                            continue;
                        }
                    }
                    let dl = lib.get("downloads");
                    let artifact = dl
                        .and_then(|d| d.get("artifact"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    if let Some(p) = crate::fetch::lib_path_of(&artifact) {
                        let a_url = artifact
                            .get("url")
                            .and_then(|x| x.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let lib_url = lib.get("url").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                        let url = final_lib_url(&name, &a_url, &lib_url);
                        if !url.is_empty() {
                            jobs.push((
                                url,
                                lib_root.join(&p),
                                artifact.get("sha1").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                                artifact.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                            ));
                        }
                    } else if !name.is_empty() {
                        let lib_url = lib.get("url").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                        jobs.push((
                            final_lib_url(&name, "", &lib_url),
                            lib_root.join(maven_path(&name)),
                            String::new(),
                            0,
                        ));
                    }
                }
            }
        };
        collect(&v);
        // install_profile 里的 processors 库在 libraries 同级（点分库列表）
        if entry_name == "install_profile.json" {
            if let Some(libs) = v.get("libraries").and_then(|x| x.as_array()) {
                let _ = libs;
            }
        }
    }
    // 去重（按目标路径）
    jobs.sort_by(|a, b| a.1.cmp(&b.1));
    jobs.dedup_by(|a, b| a.1 == b.1);
    println!("安装器 libraries：{} 个", jobs.len());
    let (f, s, _) = fetch::download_batch(client, jobs, "installer libs").await?;
    println!("安装器 libraries：新下 {f}，跳过 {s}");
    Ok(())
}

/// forge/neoforge 自产库只在自家 maven：GA 精确覆盖（声明 URL 常错指
/// libraries.minecraft.net，404 "The specified blob does not exist" 即此因）。
fn final_lib_url(name: &str, artifact_url: &str, lib_declared_url: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    let ga = parts.first().copied().unwrap_or("");
    let rel = maven_rel_slash(name);
    if ga == "net.minecraftforge" && parts.get(1).copied() == Some("forge") {
        return format!("https://maven.minecraftforge.net/{rel}");
    }
    if ga == "net.neoforged" {
        return format!("https://maven.neoforged.net/releases/{rel}");
    }
    if !lib_declared_url.is_empty() {
        return format!("{}/{rel}", lib_declared_url.trim_end_matches('/'));
    }
    if !artifact_url.is_empty() {
        return artifact_url.to_string();
    }
    crate::profile::maven_url(name)
}

fn maven_rel_slash(name: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return name.to_string();
    }
    let (g, a, ver) = (parts[0], parts[1], parts[2]);
    let classifier = parts.get(3).copied().unwrap_or("");
    let file = if classifier.is_empty() {
        format!("{a}-{ver}.jar")
    } else {
        format!("{a}-{ver}-{classifier}.jar")
    };
    format!("{}/{a}/{ver}/{file}", g.replace('.', "/"))
}

fn maven_path(name: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return name.replace(':', "_");
    }
    let (g, a, ver) = (parts[0], parts[1], parts[2]);
    let classifier = parts.get(3).copied().unwrap_or("");
    let file = if classifier.is_empty() {
        format!("{a}-{ver}.jar")
    } else {
        format!("{a}-{ver}-{classifier}.jar")
    };
    format!("{}/{a}/{ver}/{file}", g.replace('.', "/")).replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn required_java_major(data: &Path, mc: &str) -> Option<u32> {
    let p = fetch::versions_dir(data).join(mc).join(format!("{mc}.json"));
    let text = std::fs::read_to_string(p).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.pointer("/java_version/majorVersion").and_then(|x| x.as_u64()).map(|x| x as u32)
}

use std::io::Read as _;
