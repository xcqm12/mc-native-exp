//! mc-native-exp：实验项目——官方全量下载 Minecraft 文件 + 微软正版账号认证，
//! 然后用 Rust 手写 class 解析器与字节码解释器原型，真刀真枪尝试「启动时转译」，
//! 用数据回答：脱离 JVM 启动 Java 版 Minecraft，到底差在哪、差多远。
//!
//! 下载物只做 zip/字节级解析，绝不执行；认证用用户自己的正版账号，不绕过。

#![allow(dead_code)]

mod auth;
mod classparse;
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
    /// 数据目录（下载物 / token / 报告数据），默认 <exe目录>\data
    #[arg(long, default_value = "data")]
    data: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
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
    let res = rt.block_on(run(cli));
    if let Err(e) = res {
        eprintln!("错误：{e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let data = if cli.data.is_absolute() {
        cli.data
    } else {
        // 相对路径基于当前工作目录（标准 CLI 语义）
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&cli.data)
    };
    std::fs::create_dir_all(&data).map_err(|e| format!("创建数据目录失败：{e}"))?;

    let client = reqwest::Client::builder()
        .user_agent("mc-native-exp/0.1 (experimental; personal use)")
        // 部分站点（宝塔 nginx）h2 异常：本实验统一走 http/1.1
        .http1_only()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client：{e}"))?;

    match cli.cmd {
        Cmd::Auth => {
            auth::login(&client, &data).await?;
            Ok(())
        }
        Cmd::Manifest => manifest::fetch(&client, &data).await,
        Cmd::Fetch { version, client_only } => {
            fetch::fetch_version(&client, &data, version.as_deref(), client_only).await
        }
        Cmd::FetchAllClients { limit } => fetch::fetch_all_clients(&client, &data, limit).await,
        Cmd::Scan { version, all } => scan::run(&data, version.as_deref(), all),
        Cmd::Interp { version } => interp::run(&data, version.as_deref()),
        Cmd::Report => report::run(&data),
        Cmd::Loaders => {
            println!("实验支持的加载器：{}", loaders::SUPPORTED.join(" / "));
            println!("说明：Fabric/Quilt 走 meta API 直装（无需 Java）；NeoForge/Forge 走官方安装器 + 自动下载 JRE。");
            println!("OptiFine/LiteLoader（补丁型/停更）不在自动安装范围。");
            Ok(())
        }
        Cmd::Install { loader, mc, version } => {
            let id = loaders::install(&client, &data, &loader, &mc, version.as_deref()).await?;
            println!("版本 id：{id}");
            println!("启动：mc-native-exp launch {id}");
            Ok(())
        }
        Cmd::Jre { major } => {
            let p = jre::ensure(&client, &data, major).await?;
            println!("JRE 就绪：{}", p.display());
            Ok(())
        }
        Cmd::Launch { version_id, mem, smoke } => launch::run(&client, &data, &version_id, mem, smoke).await,
    }
}
