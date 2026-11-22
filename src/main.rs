//! mc-native-exp：实验项目——官方全量下载 Minecraft 文件 + 微软正版账号认证，
//! 然后用 Rust 手写 class 解析器与字节码解释器原型，真刀真枪尝试「启动时转译」，
//! 用数据回答：脱离 JVM 启动 Java 版 Minecraft，到底差在哪、差多远。
//!
//! 下载物只做 zip/字节级解析，绝不执行；认证用用户自己的正版账号，不绕过。

#![allow(dead_code)]

mod auth;
mod classparse;
mod gui;
mod theme;
mod fetch;
mod interp;
mod jre;
mod launch;
mod loaders;
mod manifest;
mod profile;
mod report;
mod scan;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "mc-native-exp",
    about = "实验：官方全量下载 MC + 正版认证 + Rust 字节码解释器原型（脱离 JVM 可行性）"
)]
struct Cli {
    /// 数据目录（下载物 / token / 报告数据）；省略时固定为 <exe目录>\data
    #[arg(long)]
    data: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 微软正版账号登录（设备码流程），校验 MC 所有权并缓存 token
    Auth,
    /// 拉取官方版本清单（全部版本）
    Manifest,
    /// 下载指定版本的全套官方文件（client / libraries / assets / natives）
    Fetch {
        /// 版本 id（如 1.21.4）；省略 = manifest 里的最新正式版
        version: Option<String>,
        /// 只下载 version JSON + client jar（供扫描/解释，跳过 libraries/assets）
        #[arg(long)]
        client_only: bool,
    },
    /// 下载清单里全部版本的 client jar（跨版本分析用，体积数 GB）
    FetchAllClients {
        /// 只处理最新的 N 个版本（默认全部）
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// 静态分析一个（或 --all 全部已下载的）client jar：native 方法 / invokedynamic / 反射等
    Scan {
        version: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// 字节码解释器原型：从 net.minecraft.client.main.Main.main 开始解释，记录第一堵墙
    Interp {
        version: Option<String>,
    },
    /// 汇总全部数据生成 REPORT.md
    Report,
    /// 控制台交互菜单（默认双击进 GUI）
    Menu,
    /// 控制台交互菜单（默认双击进 GUI）
    /// 列出实验支持的模组加载器
    Loaders,
    /// 自动安装模组加载器（fabric/quilt/neoforge/forge）到项目 data 目录
    Install {
        /// 加载器名（fabric | quilt | neoforge | forge）
        loader: String,
        /// MC 版本（如 1.21.4）
        mc: String,
        /// 加载器版本；省略 = 最新支持版
        #[arg(long)]
        version: Option<String>,
    },
    /// 预下载指定大版本的 Temurin JRE 到 data\jre
    Jre {
        major: u32,
    },
    /// 启动已安装的版本 id（正版会话；smoke 模式默认 45 秒后收尾）
    Launch {
        version_id: String,
        /// 内存上限 MB
        #[arg(long, default_value_t = 4096)]
        mem: u32,
        /// smoke 秒数（0 = 不自动退出，交给用户玩）
        #[arg(long, default_value_t = 45)]
        smoke: u64,
    },
}

fn main() {
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let res = rt.block_on(run(cli, rt.handle().clone()));
    if let Err(e) = res {
        eprintln!("错误：{e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli, handle: tokio::runtime::Handle) -> Result<(), String> {
    // 数据目录：显式绝对路径尊重之；显式相对路径基于 CWD；
    // 未传 --data（双击启动）固定 exe 目录旁 data\，不受启动位置影响
    let data = match &cli.data {
        Some(p) if p.is_absolute() => p.clone(),
        Some(p) => std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p),
        None => std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("data"),
    };
    std::fs::create_dir_all(&data).map_err(|e| format!("创建数据目录失败：{e}"))?;

    let client = reqwest::Client::builder()
        .user_agent("mc-native-exp/0.1 (experimental; personal use)")
        // 部分站点（宝塔 nginx）h2 异常：本实验统一走 http/1.1
        .http1_only()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client：{e}"))?;

    let Some(cmd) = cli.cmd else {
        // 双击启动：进 GUI
        return gui::run_gui(handle, data);
    };
    if let Cmd::Menu = cmd {
        return interactive_menu(&client, &data).await;
    }
    run_cmd(&client, &data, cmd).await
}

/// 交互式菜单：双击启动（无参数）时进入，循环执行直到选择退出。
async fn interactive_menu(client: &reqwest::Client, data: &PathBuf) -> Result<(), String> {
    println!("mc-native-exp v{} · 交互式菜单（实验项目，可能不稳定）", env!("CARGO_PKG_VERSION"));
    println!("数据目录：{}", data.display());
    loop {
        println!();
        println!("──────────────────────────────────────────");
        println!(" [1] 正版账号登录 / 状态");
        println!(" [2] 拉取官方版本清单（全部版本）");
        println!(" [3] 下载原版全套（client/libraries/assets）");
        println!(" [4] 安装模组加载器（fabric/quilt/neoforge/forge）");
        println!(" [5] 启动已安装版本（正版会话）");
        println!(" [6] 静态扫描 client jar");
        println!(" [7] 字节码解释器实验");
        println!(" [8] 生成实验报告 REPORT.md");
        println!(" [0] 退出");
        print!("选择> ");
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let choice = line.trim();
        let res = match choice {
            "1" => auth::login(client, data).await.map(|_| ()),
            "2" => manifest::fetch(client, data).await,
            "3" => {
                let mc = ask("MC 版本（回车=最新正式版）");
                fetch::fetch_version(client, data, opt_empty(&mc).as_deref(), false).await
            }
            "4" => {
                let loader = ask("加载器 fabric/quilt/neoforge/forge（回车=fabric）");
                let loader = if loader.is_empty() { "fabric".to_string() } else { loader.to_lowercase() };
                let mc = ask("MC 版本（回车=1.21.4）");
                let mc = if mc.is_empty() { "1.21.4".to_string() } else { mc };
                let lv = ask("加载器版本（回车=最新）");
                loaders::install(client, data, &loader, &mc, opt_empty(&lv).as_deref()).await.map(|_| ())
            }
            "5" => {
                let id = ask("要启动的版本 id（回车=列出已安装）");
                let id = if id.is_empty() { list_versions(data) } else { id };
                if id.is_empty() {
                    println!("（没有已安装的版本）");
                    Ok(())
                } else {
                    launch::run(client, data, &id, 4096, 0).await
                }
            }
            "6" => {
                let v = ask("版本 id（回车=1.21.4）");
                let v = if v.is_empty() { "1.21.4".to_string() } else { v };
                scan::run(data, Some(&v), false)
            }
            "7" => {
                let v = ask("版本 id（回车=1.21.4）");
                let v = if v.is_empty() { "1.21.4".to_string() } else { v };
                interp::run(data, Some(&v))
            }
            "8" => report::run(data),
            "0" | "q" | "Q" => return Ok(()),
            other => {
                println!("无效选择：{other}");
                continue;
            }
        };
        if let Err(e) = res {
            println!("❌ {e}");
        } else {
            println!("✅ 完成");
        }
    }
    Ok(())
}

fn opt_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.trim().to_string())
}

fn ask(prompt: &str) -> String {
    print!("{prompt}> ");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line.trim().to_string()
}

fn list_versions(data: &PathBuf) -> String {
    let dir = data.join("versions");
    let mut found: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.join(e.file_name()).with_extension("json").exists() || p.is_dir() {
                found.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    for f in found.iter().enumerate() {
        println!("  [{}] {}", f.0, f.1);
    }
    if found.is_empty() {
        return String::new();
    }
    print!("输入序号选择（回车=第一个）> ");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    let t = line.trim();
    match t.parse::<usize>() {
        Ok(i) if i < found.len() => found[i].clone(),
        _ if !t.is_empty() => t.to_string(),
        _ => found.first().cloned().unwrap_or_default(),
    }
}

async fn run_cmd(
    client: &reqwest::Client,
    data: &PathBuf,
    cmd: Cmd,
) -> Result<(), String> {
    match cmd {
        Cmd::Auth => {
            auth::login(client, data).await?;
            Ok(())
        }
        Cmd::Manifest => manifest::fetch(client, data).await,
        Cmd::Fetch { version, client_only } => {
            fetch::fetch_version(client, data, version.as_deref(), client_only).await
        }
        Cmd::FetchAllClients { limit } => fetch::fetch_all_clients(client, data, limit).await,
        Cmd::Scan { version, all } => scan::run(data, version.as_deref(), all),
        Cmd::Interp { version } => interp::run(data, version.as_deref()),
        Cmd::Report => report::run(data),
        Cmd::Menu => interactive_menu(client, data).await,
        Cmd::Loaders => {
            println!("实验支持的加载器：{}", loaders::SUPPORTED.join(" / "));
            println!("说明：fabric/quilt/neoforge/forge 统一安装器路线 + 自动 JRE。");
            println!("OptiFine/LiteLoader（补丁型/停更）不在自动安装范围。");
            Ok(())
        }
        Cmd::Install { loader, mc, version } => {
            let id = loaders::install(client, data, &loader, &mc, version.as_deref()).await?;
            println!("版本 id：{id}");
            println!("启动：选择菜单 [5] 或 mc-native-exp launch {id}");
            Ok(())
        }
        Cmd::Jre { major } => {
            let p = jre::ensure(client, data, major).await?;
            println!("JRE 就绪：{}", p.display());
            Ok(())
        }
        Cmd::Launch { version_id, mem, smoke } => launch::run(client, data, &version_id, mem, smoke).await,
    }
}
