//! 官方源全量下载：version JSON（含 inheritsFrom 合并）、client jar、libraries、
//! assets（index + objects）、natives 解压。纯 piston-meta / piston-data /
//! libraries.minecraft.net / resources.download.minecraft.net，SHA1 流式校验，
//! 已存在且哈希匹配的文件跳过（断点续传语义）。
//!
//! 下载物只落盘解析，绝不执行。正版账号认证是本实验一切取用的前提。

use crate::auth;
use crate::manifest::{self, Manifest, VersionEntry};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::io::Read;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Deserialize)]
struct RawVersion {
    #[serde(default)]
    inherits_from: Option<String>,
    #[serde(default)]
    args: serde_json::Value,
    #[serde(default)]
    minecraft_arguments: Option<String>,
    #[serde(default)]
    libraries: Vec<serde_json::Value>,
    #[serde(default)]
    downloads: serde_json::Value,
    #[serde(default)]
    asset_index: Option<serde_json::Value>,
    #[serde(default)]
    java_version: Option<serde_json::Value>,
    #[serde(default)]
    main_class: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FetchSummary {
    pub version: String,
    pub files: usize,
    pub bytes: u64,
    pub skipped: usize,
    pub client_jar: Option<String>,
}

pub(crate) fn versions_dir(data: &Path) -> PathBuf {
    data.join("versions")
}

fn jars_dir(data: &Path) -> PathBuf {
    data.join("jars")
}

/// BMCLAPI 国内镜像映射（与 Aurora Launcher launch.rs 同规则）。
/// 返回镜像 URL；无映射的 host 返回 None（保持官方）。
pub(crate) fn bmclapi_mirror(url: &str, version_id: &str) -> Option<String> {
    // path 保留前导斜杠（url.path() 语义）
    let mut path = url.splitn(4, '/').nth(3).unwrap_or("").to_string();
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    let host = {
        let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
        rest.split('/').next().unwrap_or("")
    };
    let m = |suffix: &str| format!("https://bmclapi2.bangbang93.com{suffix}");
    match host {
        "piston-meta.mojang.com" | "launchermeta.mojang.com" => Some(m(&path)),
        "piston-data.mojang.com" => {
            if path.ends_with("/client.jar") && !version_id.is_empty() {
                Some(m(&format!("/version/{version_id}/client")))
            } else {
                Some(m(&path))
            }
        }
        // BMCLAPI assets 带 /assets 前缀：/0a/hash → /assets/0a/hash
        "resources.download.minecraft.net" => Some(m(&format!("/assets{path}"))),
        "libraries.minecraft.net" => Some(m(&format!("/maven{path}"))),
        "maven.minecraftforge.net" => Some(m(&format!("/maven{path}"))),
        "maven.fabricmc.net" => Some(m(&format!("/maven{path}"))),
        "maven.neoforged.net" => Some(m(&format!(
            "/maven{}",
            path.strip_prefix("/releases").unwrap_or(&path)
        ))),
        _ => None,
    }
}

/// 候选顺序：maven/meta 类国内镜像优先；assets 大流量走官方优先
/// （本网络实测 BMCLAPI assets 高并发不稳），无镜像则官方单独。
pub(crate) fn mirror_candidates(url: &str, version_id: &str) -> Vec<String> {
    let assets_first = url.contains("resources.download.minecraft.net");
    match bmclapi_mirror(url, version_id) {
        Some(m) if assets_first => vec![url.to_string(), m],
        Some(m) => vec![m, url.to_string()],
        None => vec![url.to_string()],
    }
}

/// 简单并发下载：有界并发，逐个回传进度行。
pub(crate) async fn download_batch(
    client: &reqwest::Client,
    jobs: Vec<(String, PathBuf, String, u64)>, // (url, dest, sha1, size)
    label: &str,
) -> Result<(usize, usize, u64), String> {
    let total = jobs.len();
    let mut skipped = 0usize;
    let mut fetched = 0usize;
    let mut bytes = 0u64;
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(12));
    let mut handles = Vec::new();
    for (url, dest, sha1_hex, size) in jobs {
        let permit = sem.clone().acquire_owned().await.map_err(|e| e.to_string())?;
        let client = client.clone();
        let label = label.to_string();
        handles.push(tokio::spawn(async move {
            let r = download_one(&client, &url, &dest, &sha1_hex, size, &label).await;
            drop(permit);
            r
        }));
    }
    for h in handles {
        let (did_fetch, nbytes) = h.await.map_err(|e| format!("下载任务崩溃：{e}"))??;
        if did_fetch {
            fetched += 1;
            bytes += nbytes;
        } else {
            skipped += 1;
        }
    }
    let _ = total;
    Ok((fetched, skipped, bytes))
}

/// 单文件：dest 已存在且 size 匹配则校验 SHA1，通过即跳过；否则流式下载校验。
/// 返回 (是否新下载, 字节数)。
pub(crate) async fn download_one(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    sha1_hex: &str,
    size: u64,
    label: &str,
) -> Result<(bool, u64), String> {
    // 镜像优先（BMCLAPI），失败回落官方
    let candidates = mirror_candidates(url, "");
    let mut last = String::new();
    for cand in &candidates {
        match download_one_from(client, cand, dest, sha1_hex, size, label).await {
            Ok(r) => return Ok(r),
            Err(e) => {
                if candidates.len() > 1 {
                    eprintln!(
                        "  {label}：{} 失败（{e}），换源重试……",
                        cand.split('/').nth(2).unwrap_or("?")
                    );
                }
                last = e;
            }
        }
    }
    Err(last)
}

async fn download_one_from(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    sha1_hex: &str,
    size: u64,
    label: &str,
) -> Result<(bool, u64), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
    }
    // 已有文件：size 匹配 → 校验 sha1；通过 = 跳过。
    if size > 0 {
        if let Ok(meta) = std::fs::metadata(dest) {
            if meta.len() == size {
                if sha1_hex.is_empty() {
                    return Ok((false, 0));
                }
                if verify_sha1(dest, sha1_hex)? {
                    return Ok((false, 0));
                }
            }
        }
    }
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2 * attempt as u64)).await;
        }
        match fetch_to_file(client, url, dest, sha1_hex).await {
            Ok(nbytes) => return Ok((true, nbytes)),
            Err(e) => {
                let status_like = e.contains("HTTP 4");
                if status_like {
                    return Err(format!("{label}：{e}"));
                }
                eprintln!("  重试 {}/2（{}）：{}", attempt + 1, label, e);
            }
        }
    }
    Err(format!("{label}：下载重试耗尽"))
}

async fn fetch_to_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    sha1_hex: &str,
) -> Result<u64, String> {
    // 断点续传：.part 已有内容则带 Range 续传（弱链路多次重试可收敛）
    let tmp = dest.with_extension("part");
    let mut hasher = Sha1::new();
    let mut start = 0u64;
    let mut req = client.get(url);
    if tmp.exists() {
        start = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        if start > 0 {
            req = req.header("Range", format!("bytes={start}-"));
            let mut f = std::fs::File::open(&tmp).map_err(|e| format!("{e}"))?;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = f.read(&mut buf).map_err(|e| format!("{e}"))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }
    }
    let resp = req.send().await.map_err(|e| format!("HTTP {e}"))?;
    let status = resp.status();
    let content_length = resp.content_length().unwrap_or(0);
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut file;
    if start > 0 && status.as_u16() == 206 {
        file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&tmp)
            .await
            .map_err(|e| format!("打开续传文件失败：{e}"))?;
    } else {
        if start > 0 {
            // 服务端不支持 Range（200 全量）：从头重来
            hasher = Sha1::new();
            start = 0;
        }
        if status.as_u16() == 404 || status.as_u16() == 410 {
            // 目标不存在：保留 .part 无意义
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("HTTP {status}"));        }
        file = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| format!("创建临时文件失败：{e}"))?;
    }
    let mut nbytes = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("传输中断：{e}"))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| format!("写盘失败：{e}"))?;
        nbytes += chunk.len() as u64;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    // 206：content_length 是本次增量长度；200：是全量。不符 = 传输不完整（保留 .part 续传）
    let partial = status.as_u16() == 206;
    if content_length > 0 && nbytes != content_length {
        let total = if partial { start + nbytes } else { nbytes };
        return Err(format!(
            "传输不完整（{total}/{content_length} B，增量 {nbytes}）"
        ));
    }
    let got = hex::encode(hasher.finalize());
    if !sha1_hex.is_empty() && got != sha1_hex {
        // 哈希不符 = .part 已不可信（Range/全量混写等）：删除，下轮全新下载
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("SHA1 不匹配（期望 {sha1_hex}，得到 {got}）"));
    }
    std::fs::rename(&tmp, dest).map_err(|e| format!("改名失败：{e}"))?;
    Ok(nbytes)
}

fn verify_sha1(path: &Path, expect: &str) -> Result<bool, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{e}"))?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()) == expect)
}

/// 拉取并落盘 version JSON（含 inheritsFrom 递归，深度 ≤4）。
async fn load_version_json(
    client: &reqwest::Client,
    entry: &VersionEntry,
    data: &Path,
) -> Result<(PathBuf, serde_json::Value), String> {
    let dir = versions_dir(data).join(&entry.id);
    let path = dir.join(format!("{}.json", entry.id));
    if !path.exists() {
        let resp = client
            .get(&entry.url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("拉取版本 JSON 失败：{e}"))?;
        let bytes = resp.bytes().await.map_err(|e| format!("{e}"))?.to_vec();
        manifest::write_atomic(&path, &bytes)?;
    }
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("{e}"))?)
            .map_err(|e| format!("版本 JSON 解析失败：{e}"))?;
    Ok((dir, v))
}

/// 下载指定版本全套（或仅 client）官方文件。需正版会话。
pub async fn fetch_version(
    client: &reqwest::Client,
    data: &Path,
    version: Option<&str>,
    client_only: bool,
) -> Result<(), String> {
    let session = auth::ensure_session(client, data).await?;
    println!("正版会话：{}", session.player_name);
    let m: Manifest = manifest::load(data)?;
    let entry = manifest::resolve(&m, version)?;
    println!("目标版本：{}（{}，{}）", entry.id, entry.kind, entry.release_time);

    let (dir, vjson) = load_version_json(client, entry, data).await?;

    // client jar
    let client_meta = vjson
        .pointer("/downloads/client")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let (client_url, client_sha, client_size) = if client_meta.is_null() {
        (String::new(), String::new(), 0u64)
    } else {
        (
            client_meta
                .get("url")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            client_meta
                .get("sha1")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            client_meta.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        )
    };
    if !client_url.is_empty() {
        let dest = dir.join(format!("{}.jar", entry.id));
        println!("下载 client jar（{} MB）……", client_size / 1024 / 1024);
        let (f, _, b) = download_batch(
            client,
            vec![(client_url.clone(), dest.clone(), client_sha.clone(), client_size)],
            &format!("client {}", entry.id),
        )
        .await?;
        println!("client jar：新下 {f}，跳过 {}", 1 - f);
        let _ = b;
    }

    if client_only {
        println!("--client-only：跳过 libraries / assets");
        return Ok(());
    }

    // libraries（windows 规则过滤 + natives 分类器）
    let mut jobs = Vec::new();
    let libs = vjson.get("libraries").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let lib_root = data.join("libraries");
    let mut native_jars = Vec::new();
    for lib in &libs {
        if !rules_allow_windows(lib.get("rules")) {
            continue;
        }
        let dl = lib.get("downloads");
        let artifact = dl
            .and_then(|d| d.get("artifact"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if let Some(p) = lib_path_of(&artifact) {
            let url = artifact.get("url").and_then(|x| x.as_str()).unwrap_or_default();
            if !url.is_empty() {
                jobs.push((
                    url.to_string(),
                    lib_root.join(&p),
                    artifact.get("sha1").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    artifact.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                ));
            }
        }
        let natives = dl
            .and_then(|d| d.get("classifiers"))
            .and_then(|c| c.get("natives-windows"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if let Some(p) = lib_path_of(&natives) {
            let url = natives.get("url").and_then(|x| x.as_str()).unwrap_or_default();
            if !url.is_empty() {
                let dest = lib_root.join(&p);
                native_jars.push(dest.clone());
                jobs.push((
                    url.to_string(),
                    dest,
                    natives.get("sha1").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    natives.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                ));
            }
        }
    }
    println!("libraries：{} 个（含 natives {}）", jobs.len(), native_jars.len());
    let (f, s, b) = download_batch(client, jobs, "libraries").await?;
    println!("libraries：新下 {f}，跳过 {s}，{} MB", b / 1024 / 1024);

    // assets
    if let Some(ai) = vjson.get("assetIndex") {
        let idx_url = ai.get("url").and_then(|x| x.as_str()).unwrap_or_default();
        if !idx_url.is_empty() {
            let idx_id = ai.get("id").and_then(|x| x.as_str()).unwrap_or("legacy");
            let idx_path = data.join("assets").join("indexes").join(format!("{idx_id}.json"));
            let (f, _, _) = download_batch(
                client,
                vec![(
                    idx_url.to_string(),
                    idx_path.clone(),
                    ai.get("sha1").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    ai.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                )],
                "asset index",
            )
            .await?;
            let _ = f;
            let idx: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&idx_path).map_err(|e| format!("{e}"))?,
            )
            .map_err(|e| format!("asset index 解析失败：{e}"))?;
            let objects = idx.get("objects").and_then(|x| x.as_array().map(|_| x)).map(|_| ());
            let _ = objects;
            let mut ajobs = Vec::new();
            if let Some(map) = idx.get("objects").and_then(|x| x.as_object()) {
                for (_name, obj) in map {
                    let hash = obj.get("hash").and_then(|x| x.as_str()).unwrap_or_default();
                    let size = obj.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
                    if hash.len() < 2 {
                        continue;
                    }
                    ajobs.push((
                        format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash),
                        data.join("assets").join("objects").join(&hash[..2]).join(hash),
                        hash.to_string(),
                        size,
                    ));
                }
            }
            println!("assets：{} 个对象（{} MB）……", ajobs.len(), {
                let t: u64 = ajobs.iter().map(|j| j.3).sum();
                t / 1024 / 1024
            });
            let (f, s, b) = download_batch(client, ajobs, "assets").await?;
            println!("assets：新下 {f}，跳过 {s}，{} MB", b / 1024 / 1024);
        }
    }

    // natives 解压（到 versions/<id>/natives）
    if !native_jars.is_empty() {
        let nats = dir.join("natives");
        std::fs::create_dir_all(&nats).map_err(|e| format!("{e}"))?;
        let mut count = 0usize;
        for jar in &native_jars {
            if !jar.exists() {
                continue;
            }
            let f = std::fs::File::open(jar).map_err(|e| format!("{e}"))?;
            let mut z = zip::ZipArchive::new(f).map_err(|e| format!("natives jar 打开失败：{e}"))?;
            for i in 0..z.len() {
                let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
                let name = entry.name().to_string();
                // 只抽 .dll（含子目录内的），META-INF 等自动被后缀过滤排除。
                if !name.to_lowercase().ends_with(".dll") {
                    continue;
                }
                let out_name = name.rsplit('/').next().unwrap_or(&name).to_string();
                let out = nats.join(out_name);
                let mut w = std::fs::File::create(&out).map_err(|e| format!("{e}"))?;
                std::io::copy(&mut entry, &mut w).map_err(|e| format!("{e}"))?;
                count += 1;
            }
        }
        println!("natives：解压 {} 个 dll 到 {}", count, nats.display());
    }

    println!("完成：版本 {} 已就绪（{}）", entry.id, dir.display());
    Ok(())
}

pub(crate) fn lib_path_of(artifact: &serde_json::Value) -> Option<String> {
    artifact
        .get("path")
        .and_then(|x| x.as_str())
        .map(|s| s.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Windows 规则过滤（与官方 launcher 语义一致：allow/deny 按 os.name 匹配）。
pub(crate) fn rules_allow_windows(rules: Option<&serde_json::Value>) -> bool {
    let Some(rules) = rules.and_then(|r| r.as_array()) else {
        return true;
    };
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        let action = rule.get("action").and_then(|x| x.as_str()).unwrap_or("allow");
        let os = rule.get("os").and_then(|o| o.get("name")).and_then(|x| x.as_str());
        let matches = match os {
            None => true,
            Some(n) => n == "windows",
        };
        if matches {
            allowed = action == "allow";
        }
    }
    allowed
}

/// 下载全部版本的 client jar（data\jars\<id>.jar），供跨版本分析。
pub async fn fetch_all_clients(
    client: &reqwest::Client,
    data: &Path,
    limit: usize,
) -> Result<(), String> {
    let session = auth::ensure_session(client, data).await?;
    println!("正版会话：{}", session.player_name);
    let m: Manifest = manifest::load(data)?;
    let mut entries: Vec<&VersionEntry> = m.versions.iter().collect();
    // 按发布时间倒序（新 → 旧），limit=0 表示全部。
    entries.sort_by(|a, b| b.release_time.cmp(&a.release_time));
    if limit > 0 {
        entries.truncate(limit);
    }
    println!("计划下载 {} 个版本的 client jar 到 data\\jars\\（数 GB，稍等）", entries.len());
    let jars = jars_dir(data);
    std::fs::create_dir_all(&jars).map_err(|e| format!("{e}"))?;
    let mut jobs = Vec::new();
    for e in &entries {
        let dest = jars.join(format!("{}.jar", e.id));
        if dest.exists() {
            continue;
        }
        // 需要先拿版本 JSON 里的 downloads.client.url
        jobs.push((*e).clone());
    }
    let mut dl_jobs = Vec::new();
    for (i, e) in jobs.iter().enumerate() {
        if i % 50 == 0 {
            println!("解析版本 JSON {}/{}……", i, jobs.len());
        }
        let (_, vjson) = match load_version_json(client, e, data).await {
            Ok(x) => x,
            Err(err) => {
                eprintln!("  跳过 {}：{err}", e.id);
                continue;
            }
        };
        let c = vjson.pointer("/downloads/client").cloned().unwrap_or(serde_json::Value::Null);
        let url = c.get("url").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        if url.is_empty() {
            continue;
        }
        dl_jobs.push((
            url,
            jars.join(format!("{}.jar", e.id)),
            c.get("sha1").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            c.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        ));
    }
    println!("开始下载 {} 个 client jar……", dl_jobs.len());
    let (f, s, b) = download_batch(client, dl_jobs, "clients").await?;
    println!("clients：新下 {f}，跳过 {s}，共 {} MB", b / 1024 / 1024);
    Ok(())
}

/// 已下载的 client jar 路径（版本 id → jars\<id>.jar 或 versions\<id>\<id>.jar）。
pub fn client_jar_path(data: &Path, version: &str) -> Option<PathBuf> {
    let in_jars = jars_dir(data).join(format!("{version}.jar"));
    if in_jars.exists() {
        return Some(in_jars);
    }
    let in_versions = versions_dir(data).join(version).join(format!("{version}.jar"));
    in_versions.exists().then_some(in_versions)
}
