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
        "fabric" | "quilt" => install_meta(client, data, loader, mc, loader_version).await,
        "forge" | "neoforge" => install_forge_like(client, data, loader, mc, loader_version).await,
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

/// 最新可用 loader 版本（meta 列表第一个 stable）。
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
        let src = it.pointer("/loader/version").and_then(|x| x.as_str());
        let src = src.or_else(|| it.pointer("/loader/version").and_then(|x| x.as_str()));
        if let Some(v) = src {
            let stable = it
                .pointer("/loader/stable")
                .and_then(|x| x.as_bool())
                .unwrap_or(true);
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

/// Fabric/Quilt：meta profile JSON → versions/<id>/ → libraries 下载（无需 Java）。
async fn install_meta(
    client: &reqwest::Client,
    data: &Path,
    loader: &str,
    mc: &str,
    loader_version: Option<&str>,
) -> Result<String, String> {
    let lv = match loader_version {
        Some(v) => v.to_string(),
        None => latest_loader_version(client, loader, mc).await?,
    };
    println!("{loader} 版本 {lv}（MC {mc}）——meta 直装");
    // inheritsFrom 的父（原版 JSON）必须先就位
    fetch::fetch_version(client, data, Some(mc), true).await?;
    let url = format!("{}/{mc}/{lv}/profile", meta_base(loader));
    let profile: Value = client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("profile 获取失败：{e}（{loader} 可能还不支持 {mc}）"))?
        .json()
        .await
        .map_err(|e| format!("{e}"))?;
    let id = profile
        .get("id")
        .and_then(|x| x.as_str())
        .ok_or("profile 缺少 id")?
        .to_string();

    let dir = fetch::versions_dir(data).join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{e}"))?;
    let path = dir.join(format!("{id}.json"));
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|e| e.to_string())?;
    crate::manifest::write_atomic(&path, &bytes)?;
    println!("版本 JSON：{}", path.display());

    crate::profile::fetch_profile(client, data, &id).await?;
    Ok(id)
}

/* ── Forge / NeoForge：官方安装器 + 自动 JRE ───────────────── */

fn neoforge_maven_meta(mc: &str) -> String {
    format!("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge")
}

/// NeoForge 最新版（maven metadata：版本号形如 <mc>.<build>）。
async fn latest_neoforge(client: &reqwest::Client, mc: &str) -> Result<String, String> {
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

/// Forge 最新版（promotions API：1.21.4 → "51.0.x"）。
async fn latest_forge(client: &reqwest::Client, mc: &str) -> Result<String, String> {
    let v: Value = client
        .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Forge promotions：{e}"))?
        .json()
        .await
        .map_err(|e| format!("{e}"))?;
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
async fn install_forge_like(
    client: &reqwest::Client,
    data: &Path,
    loader: &str,
    mc: &str,
    loader_version: Option<&str>,
) -> Result<String, String> {
    let lv = match loader_version {
        Some(v) => v.to_string(),
        None => {
            if loader == "neoforge" {
                latest_neoforge(client, mc).await?
            } else {
                latest_forge(client, mc).await?
            }
        }
    };
    println!("{loader} 版本 {lv}（MC {mc}）——安装器安装");

    // 0) 原版 JSON 先就位（安装器要 inheritsFrom 的父版本）
    let m = crate::manifest::load(data)?;
    let entry = crate::manifest::resolve(&m, Some(mc))?;
    fetch::fetch_version(client, data, Some(mc), true).await?;
    let _ = entry;

    // 1) 安装器
    let url = installer_url(client, loader, mc, &lv).await?;
    let dl_dir = data.join("downloads");
    std::fs::create_dir_all(&dl_dir).map_err(|e| format!("{e}"))?;
    let installer = dl_dir.join(format!("{loader}-{lv}-installer.jar"));
    download_file(client, &url, &installer, &format!("{loader} installer")).await?;

    // 2) 预下载安装器要用的 libraries（读 install_profile.json/version.json）
    predownload_installer_libs(client, data, &installer).await?;

    // 3) JRE（安装器处理器需要）
    let java_major = required_java_major(data, mc).unwrap_or(21);
    let java = jre::ensure(client, data, java_major).await?;
    println!("安装器用 JRE：{}", java.display());

    // 4) --installClient 到 data\game
    let game_dir = data.join("game");
    std::fs::create_dir_all(&game_dir).map_err(|e| format!("{e}"))?;
    let out = tokio::process::Command::new(&java)
        .args(["-jar"])
        .arg(&installer)
        .arg("--installClient")
        .arg(&game_dir)
        .current_dir(&game_dir)
        .output()
        .await
        .map_err(|e| format!("安装器启动失败：{e}"))?;
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log_path = data.join(format!("{loader}-install.log"));
    let _ = std::fs::write(&log_path, &log);
    if !out.status.success() {
        let tail: String = log.chars().rev().take(2000).collect::<String>().chars().rev().collect();
        return Err(format!(
            "安装器退出码 {:?}，日志尾部：{}",
            out.status.code(),
            tail
        ));
    }

    // 5) 找生成的版本 id（versions 目录里带 loader 前缀的新 JSON）
    let id = find_installed(data, &game_dir, loader, &lv)?;
    println!("安装完成：{id}");
    crate::profile::fetch_profile(client, data, &id).await?;
    Ok(id)
}

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
                    let dl = lib.get("downloads");
                    let artifact = dl
                        .and_then(|d| d.get("artifact"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    if let Some(p) = crate::fetch::lib_path_of(&artifact) {
                        let url = artifact
                            .get("url")
                            .and_then(|x| x.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let url = if url.is_empty() { crate::profile::maven_url(&name) } else { url };
                        if !url.is_empty() {
                            jobs.push((
                                url,
                                lib_root.join(&p),
                                artifact.get("sha1").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                                artifact.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                            ));
                        }
                    } else if !name.is_empty() {
                        // 只有 maven 坐标（老版 Forge）：路径推出来直接下
                        let p = maven_path(&name);
                        jobs.push((
                            crate::profile::maven_url(&name),
                            lib_root.join(p),
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
