//! 本地 profile（模组加载器版本 JSON）的全套文件下载与合并视图。
//!
//! loader 安装后生成 versions/<id>/<id>.json（含 inheritsFrom 指向原版），
//! 本模块负责：递归合并 → 下载 client / libraries / assets / natives →
//! 为 launch 提供合并后的版本视图。

use crate::fetch;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 递归合并 inheritsFrom：parent 为底，child 覆盖标量字段、libraries 拼接、
/// args.game 拼接（jvm 子级覆盖）。
pub fn merged_version(data: &Path, id: &str, depth: usize) -> Result<Value, String> {
    let path = fetch::versions_dir(data).join(id).join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("版本 JSON 不存在：{id}（先 fetch 或 install）"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{id} JSON 解析失败：{e}"))?;
    if depth >= 4 {
        return Ok(v);
    }
    let Some(parent_id) = v.get("inheritsFrom").and_then(|x| x.as_str()) else {
        return Ok(v);
    };
    let parent = merged_version(data, parent_id, depth + 1)?;
    Ok(merge_version(&parent, &v))
}

fn merge_version(parent: &Value, child: &Value) -> Value {
    let mut out = parent.clone();
    if let Some(map) = child.as_object() {
        for (k, val) in map {
            match k.as_str() {
                "inheritsFrom" => continue,
                "libraries" => {
                    let mut libs = out
                        .get("libraries")
                        .and_then(|x| x.as_array())
                        .cloned()
                        .unwrap_or_default();
                    libs.extend(val.as_array().cloned().unwrap_or_default());
                    out["libraries"] = Value::Array(libs);
                }
                "args" => {
                    let mut game = out
                        .pointer("/args/game")
                        .and_then(|x| x.as_array())
                        .cloned()
                        .unwrap_or_default();
                    game.extend(
                        val.pointer("/game")
                            .and_then(|x| x.as_array())
                            .cloned()
                            .unwrap_or_default(),
                    );
                    let jvm = val
                        .pointer("/jvm")
                        .cloned()
                        .or_else(|| out.pointer("/args/jvm").cloned())
                        .unwrap_or(Value::Null);
                    out["args"] = serde_json::json!({ "game": game, "jvm": jvm });
                }
                _ => {
                    out[k] = val.clone();
                }
            }
        }
    }
    out
}

/// 下载本地 profile 全套：client（含继承父）、libraries（空 url 走 maven 兜底）、
/// assets、natives 解压。
pub async fn fetch_profile(client: &reqwest::Client, data: &Path, id: &str) -> Result<(), String> {
    let v = merged_version(data, id, 0)?;
    let dir = fetch::versions_dir(data).join(id);
    let mut jobs: Vec<(String, PathBuf, String, u64)> = Vec::new();

    // client jar：继承链上每个有 downloads.client 的版本各一份
    let mut chain = vec![v.clone()];
    let mut cur = v.clone();
    while let Some(pid) = cur.get("inheritsFrom").and_then(|x| x.as_str()).map(String::from) {
        let p = fetch::versions_dir(data).join(&pid).join(format!("{pid}.json"));
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{e}"))?;
        cur = serde_json::from_str(&text).map_err(|e| format!("{e}"))?;
        chain.push(cur.clone());
    }
    for node in &chain {
        let c = node.pointer("/downloads/client").cloned().unwrap_or(Value::Null);
        if c.is_null() {
            continue;
        }
        let url = c.get("url").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        if url.is_empty() {
            continue;
        }
        let base = node.get("id").and_then(|x| x.as_str()).unwrap_or(id).to_string();
        jobs.push((
            url,
            fetch::versions_dir(data).join(&base).join(format!("{base}.jar")),
            c.get("sha1").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            c.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        ));
    }

    // libraries
    let lib_root = data.join("libraries");
    let mut native_jars = Vec::new();
    if let Some(libs) = v.get("libraries").and_then(|x| x.as_array()) {
        for lib in libs {
            if !fetch::rules_allow_windows(lib.get("rules")) {
                continue;
            }
            let dl = lib.get("downloads");
            let artifact = dl
                .and_then(|d| d.get("artifact"))
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(p) = fetch::lib_path_of(&artifact) {
                let mut url = artifact
                    .get("url")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string();
                if url.is_empty() {
                    // Forge/NeoForge 部分库无 url：按 maven 坐标兜底
                    url = maven_url(lib.get("name").and_then(|x| x.as_str()).unwrap_or_default());
                }
                if !url.is_empty() {
                    jobs.push((
                        url,
                        lib_root.join(&p),
                        artifact
                            .get("sha1")
                            .and_then(|x| x.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        artifact.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                    ));
                }
            }
            let natives = dl
                .and_then(|d| d.get("classifiers"))
                .and_then(|c| c.get("natives-windows"))
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(p) = fetch::lib_path_of(&natives) {
                let url = natives
                    .get("url")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string();
                if !url.is_empty() {
                    let dest = lib_root.join(&p);
                    native_jars.push(dest.clone());
                    jobs.push((
                        url,
                        dest,
                        natives
                            .get("sha1")
                            .and_then(|x| x.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        natives.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                    ));
                }
            }
        }
    }

    // assets（合并视图顶层的 assetIndex）
    if let Some(ai) = v.get("assetIndex") {
        let idx_url = ai.get("url").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        if !idx_url.is_empty() {
            let idx_id = ai.get("id").and_then(|x| x.as_str()).unwrap_or("legacy").to_string();
            let idx_path = data.join("assets").join("indexes").join(format!("{idx_id}.json"));
            if !idx_path.exists() {
                let (f, _, _) = fetch::download_batch(
                    client,
                    vec![(
                        idx_url.clone(),
                        idx_path.clone(),
                        ai.get("sha1").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                        ai.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                    )],
                    "asset index",
                )
                .await?;
                let _ = f;
            }
            if let Ok(text) = std::fs::read_to_string(&idx_path) {
                if let Ok(idx) = serde_json::from_str::<Value>(&text) {
                    if let Some(map) = idx.get("objects").and_then(|x| x.as_object()) {
                        for (_name, obj) in map {
                            let hash = obj
                                .get("hash")
                                .and_then(|x| x.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let size = obj.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
                            if hash.len() < 2 {
                                continue;
                            }
                            jobs.push((
                                format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash),
                                data.join("assets").join("objects").join(&hash[..2]).join(&hash),
                                hash.clone(),
                                size,
                            ));
                        }
                    }
                }
            }
        }
    }

    println!("profile {id}：待下载 {} 个文件……", jobs.len());
    let (f, s, b) = fetch::download_batch(client, jobs, &format!("profile {id}")).await?;
    println!("profile {id}：新下 {f}，跳过 {s}，{} MB", b / 1024 / 1024);

    // natives 解压
    if !native_jars.is_empty() {
        let nats = dir.join("natives");
        std::fs::create_dir_all(&nats).map_err(|e| format!("{e}"))?;
        let mut count = 0usize;
        for jar in &native_jars {
            if !jar.exists() {
                continue;
            }
            let fp = std::fs::File::open(jar).map_err(|e| format!("{e}"))?;
            let mut z = zip::ZipArchive::new(fp).map_err(|e| format!("{e}"))?;
            for i in 0..z.len() {
                let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
                let name = entry.name().to_string();
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
        println!("natives：解压 {count} 个 dll");
    }
    Ok(())
}

/// maven 坐标（group:artifact:version）→ libraries.minecraft.net URL。
pub fn maven_url(name: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return String::new();
    }
    let (g, a, ver) = (parts[0], parts[1], parts[2]);
    let path = format!("{}/{a}/{ver}/{a}-{ver}.jar", g.replace('.', "/"));
    format!("https://libraries.minecraft.net/{path}")
}
