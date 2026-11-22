//! 游戏启动（模组版/普通版走 Java；vanilla 的原生解释器实验见 interp.rs）：
//! 合并版本视图 → 确保全套文件 → 拼 classpath / natives / 参数（正版会话）→
//! spawn java → 日志落盘 + smoke 模式（N 秒后检查启动标记并退出）。
//!
//! 启动优化：默认 G1 + StringDeduplication、并行下载已由 fetch 层覆盖、
//! 已存在且哈希匹配的文件零开销跳过（二次启动近似瞬时）。

use crate::auth;
use crate::profile;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

pub async fn run(
    client: &reqwest::Client,
    data: &Path,
    id: &str,
    mem_mb: u32,
    smoke_secs: u64,
) -> Result<(), String> {
    let session = auth::ensure_session(client, data).await?;
    println!("正版会话：{}", session.player_name);

    let v = profile::merged_version(data, id, 0)?;
    let dir = crate::fetch::versions_dir(data).join(id);

    // 文件确保（已存在的哈希匹配即跳过）
    profile::fetch_profile(client, data, id).await?;

    // natives 目录（profile fetch 已解压；没有则建空目录占位）
    let natives = dir.join("natives");
    std::fs::create_dir_all(&natives).map_err(|e| format!("{e}"))?;

    // classpath：libraries（windows 规则）+ client jar。
    // 按 maven GA 去重且 loader（子 profile，列表靠后）优先——否则同库不同版本
    // 并存会触发 fabric loader 的 duplicate classes 检查直接拒绝启动。
    let mut cp = Vec::new();
    let mut seen_ga: std::collections::HashMap<String, String> = Default::default();
    if let Some(libs) = v.get("libraries").and_then(|x| x.as_array()) {
        for lib in libs.iter().rev() {
            if !crate::fetch::rules_allow_windows(lib.get("rules")) {
                continue;
            }
            let name = lib.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string();
            let parts: Vec<&str> = name.split(':').collect();
            // natives-* 分类器条目不进 classpath（dll 已解压到 natives 目录）
            if parts.len() >= 4 && parts[3].starts_with("natives") {
                continue;
            }
            let ga: String = parts.first().unwrap_or(&"").to_string()
                + ":" + parts.get(1).unwrap_or(&"");
            let version = parts.get(2).copied().unwrap_or("");
            // 同 GA 不同版本 → 子 profile 优先（跳过父版本，防 duplicate classes）；
            // 同 GA 同版本共存（forge client/universal/shim 三 jar 全必需）
            if let Some(kept) = seen_ga.get(&ga) {
                if kept != version {
                    continue;
                }
            } else {
                seen_ga.insert(ga.clone(), version.to_string());
            }
            let dl = lib.get("downloads");
            let artifact = dl.and_then(|d| d.get("artifact")).cloned().unwrap_or(Value::Null);
            let mut found = false;
            if let Some(p) = crate::fetch::lib_path_of(&artifact) {
                let full = data.join("libraries").join(&p);
                if full.exists() {
                    cp.push(full);
                    found = true;
                }
            }
            if !found {
                let p = maven_rel(&name);
                let full = data.join("libraries").join(&p);
                if full.exists() {
                    cp.push(full);
                    found = true;
                }
            }
            if !found {
                println!("  ⚠ 缺库：{name}（{}）", name.split(':').take(2).collect::<Vec<_>>().join("/"));
            }
        }
        cp.reverse(); // 恢复原顺序（父在前）
    }
    // client jar：继承链上每个有 downloads.client 的。
    // Bootstrap 系（ForgeBootstrap/BootstrapLauncher）例外：原版 jar 绝不能进
    // classpath（模块冲突），patched client 由 forge 自身 library 提供。
    let bootstrap_like = id.contains("forge") || id.contains("neoforge");
    let mut cur: Option<Value> = if bootstrap_like { None } else { Some(v.clone()) };
    let mut guard = 0;
    while cur.is_some() && guard < 5 {
        let node = cur.take().unwrap();
        let base = node.get("id").and_then(|x| x.as_str()).unwrap_or(id).to_string();
        let jar = crate::fetch::versions_dir(data).join(&base).join(format!("{base}.jar"));
        if jar.exists() {
            cp.push(jar);
        }
        cur = node
            .get("inheritsFrom")
            .and_then(|x| x.as_str())
            .map(|pid| {
                let p = crate::fetch::versions_dir(data).join(pid).join(format!("{pid}.json"));
                serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_default()).unwrap_or(Value::Null)
            });
        guard += 1;
    }
    if cp.is_empty() {
        return Err(format!("classpath 为空——{id} 的 client/libraries 未就绪"));
    }
    let classpath = cp
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(";");

    // java：data/jre 优先，找不到再系统 PATH
    let java_major = v
        .pointer("/java_version/majorVersion")
        .and_then(|x| x.as_u64())
        .unwrap_or(21) as u32;
    let java = crate::jre::ensure(client, data, java_major).await?;

    // mainClass
    let main_class = v
        .get("mainClass")
        .and_then(|x| x.as_str())
        .ok_or("版本 JSON 缺 mainClass")?
        .to_string();

    // 资产索引名
    let assets_index = v
        .get("assetIndex")
        .and_then(|x| x.get("id"))
        .and_then(|x| x.as_str())
        .unwrap_or("legacy")
        .to_string();

    let game_dir = data.join("game");
    std::fs::create_dir_all(&game_dir).map_err(|e| format!("{e}"))?;
    let assets_root = data.join("assets");

    let mut ph = |k: &str, val: &str| (format!("${{{k}}}"), val.to_string());
    let placeholders: Vec<(String, String)> = vec![
        ph("auth_player_name", &session.player_name),
        ph("auth_uuid", &session.player_uuid),
        ph("auth_access_token", &session.mc_access),
        ph("auth_session", &format!("token:{}:{}", session.mc_access, session.player_uuid)),
        ph("user_type", "msa"),
        ph("version_name", id),
        ph("version_type", "release"),
        ph("game_directory", &game_dir.to_string_lossy()),
        ph("assets_root", &assets_root.to_string_lossy()),
        ph("game_assets", &assets_root.join("virtual").join("legacy").to_string_lossy()),
        ph("assets_index_name", &assets_index),
        ph("natives_directory", &natives.to_string_lossy()),
        ph("launcher_name", "mc-native-exp"),
        ph("launcher_version", env!("CARGO_PKG_VERSION")),
        ph("classpath", &classpath),
        ph("user_properties", "{}"),
    ];
    let sub = |s: &str| -> String {
        let mut out = s.to_string();
        for (k, val) in &placeholders {
            out = out.replace(k, val);
        }
        out
    };

    // JVM 参数
    let mut jvm: Vec<String> = vec![
        format!("-Xmx{mem_mb}M"),
        "-XX:+UseG1GC".into(),
        "-XX:+UseStringDeduplication".into(),
        "-XX:MaxGCPauseMillis=37".into(),
        "-Djava.library.path=".to_string() + &natives.to_string_lossy(),
        format!("-Dio.netty.native.workdir={}", natives.to_string_lossy()),
    ];
    if let Some(arr) = v.pointer("/arguments/jvm").and_then(|x| x.as_array()) {
        for item in arr {
            match item {
                Value::String(s) => {
                    if s.contains("${classpath}") {
                        jvm.push(sub(s));
                    } else if s.starts_with("-D") || s.starts_with("-X") {
                        // 现代版本自带的一批 -D 与 -XX：原样透传（rules 粗放行）
                        jvm.push(sub(s));
                    }
                }
                Value::Object(_) => {} // 带 rules 的条目：原型按默认 features 全放行缺失跳过
                _ => {}
            }
        }
    }

    // 游戏参数
    let mut game: Vec<String> = Vec::new();
    if let Some(arr) = v.pointer("/arguments/game").and_then(|x| x.as_array()) {
        let mut expect_value = false;
        for item in arr {
            match item {
                Value::String(s) => {
                    let s = sub(s);
                    if expect_value {
                        game.push(s);
                        expect_value = false;
                    } else {
                        game.push(s);
                        // 标记下一个若是值（非 --开头）也照常压入：无需特判
                        expect_value = false;
                    }
                }
                Value::Object(o) => {
                    // 带 rules：默认 features 放行（不含 demo/resolution）
                    let allow = o
                        .get("rules")
                        .and_then(|r| r.as_array())
                        .map(|rules| {
                            rules.iter().all(|r| {
                                let action = r.get("action").and_then(|x| x.as_str()).unwrap_or("allow");
                                let feature_ok = r
                                    .get("features")
                                    .and_then(|f| f.as_object())
                                    .map(|m| m.is_empty())
                                    .unwrap_or(true);
                                action == "allow" && feature_ok
                            })
                        })
                        .unwrap_or(true);
                    if allow {
                        if let Some(Value::String(s)) = o.get("value") {
                            game.push(sub(s));
                        }
                    }
                }
                _ => {}
            }
        }
        let _ = expect_value;
    } else if let Some(legacy) = v.get("minecraft_arguments").and_then(|x| x.as_str()) {
        game.extend(legacy.split_whitespace().map(sub));
    }

    println!("jvm: {jvm:?}");
    println!("main: {main_class}");
    println!("game: {game:?}");
    let mut cmd = std::process::Command::new(&java);
    cmd.args(&jvm)
        .arg("-cp")
        .arg(&classpath)
        .arg(&main_class)
        .args(&game)
        .current_dir(&game_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    println!("启动 {id}（{}，mem {mem_mb}MB，smoke {smoke_secs}s）……", java.display());
    let mut child = cmd.spawn().map_err(|e| format!("java 进程：{e}"))?;

    let log_path = data.join(format!("launch-{id}.log"));
    let log_file = std::fs::File::create(&log_path).map_err(|e| format!("{e}"))?;
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();

    // stdout/stderr 各起线程：写日志 + 关键行打屏
    let mut handles = Vec::new();
    if let Some(out) = child.stdout.take() {
        let stop = stop2.clone();
        let mut w = log_file.try_clone().map_err(|e| format!("{e}"))?;
        handles.push(std::thread::spawn(move || {
            let rdr = std::io::BufReader::new(out);
            use std::io::BufRead as _;
            for line in rdr.lines().map_while(Result::ok) {
                let _ = writeln!(w, "{line}");
                if line.contains("Setting user:") || line.contains("Backend library: LWJGL") {
                    println!("  [启动] {line}");
                }
                if stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let mut w = log_file.try_clone().map_err(|e| format!("{e}"))?;
        let stop = stop2;
        handles.push(std::thread::spawn(move || {
            let rdr = std::io::BufReader::new(err);
            use std::io::BufRead as _;
            for line in rdr.lines().map_while(Result::ok) {
                let _ = writeln!(w, "{line}");
                if line.contains("Exception") || line.contains("ERROR") {
                    println!("  [err] {line}");
                }
                if stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
            }
        }));
    }

    if smoke_secs == 0 {
        // 交给用户玩：不自动杀进程，等游戏窗口关闭后自然结束
        println!("游戏已启动（关闭游戏窗口后本命令自然结束）");
        let status = child.wait().map_err(|e| format!("等待游戏退出失败：{e}"))?;
        for h in handles {
            let _ = h.join();
        }
        let code = status.code().unwrap_or(0);
        println!("游戏退出（code {code}），日志：{}", log_path.display());
        return if code == 0 { Ok(()) } else { std::process::exit(code) };
    }

    // smoke：等 N 秒 → 检查日志标记 → 杀进程
    std::thread::sleep(std::time::Duration::from_secs(smoke_secs));
    stop.store(true, AtomicOrdering::Relaxed);
    let _ = child.kill();
    for h in handles {
        let _ = h.join();
    }
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    // quilt 等把 log4j 输出重定向到 game/logs/latest.log（stdout 为空）
    let game_log = std::fs::read_to_string(game_dir.join("logs").join("latest.log")).unwrap_or_default();
    let combined = format!("{log}{game_log}");
    let ok_markers = ["Backend library: LWJGL", "Setting user:", "OpenAL initialized", "Sound engine started"];
    let hit = ok_markers.iter().any(|m| combined.contains(m));
    println!("smoke 判定：{}（stdout {} 行 + 游戏日志 {} 行）",
        if hit { "✅ 已进入游戏引擎初始化" } else { "⚠ 未命中启动标记" },
        log.lines().count(), game_log.lines().count());
    println!("日志：{}", log_path.display());
    if !hit {
        let tail: Vec<&str> = log.lines().rev().take(25).collect();
        println!("── 日志尾部 ──");
        for l in tail.iter().rev() {
            println!("  {l}");
        }
        std::process::exit(2);
    }
    Ok(())
}

fn maven_rel(name: &str) -> String {
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
    format!("{}/{a}/{ver}/{file}", g.replace('.', "/"))
}

use std::io::Write as _;
type PathBuf_2 = PathBuf;
#[allow(unused)]
fn _t(_: PathBuf_2) {}
