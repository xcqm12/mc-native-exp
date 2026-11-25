//! 桌面启动器样式 GUI（egui/eframe）：左侧导航栏 + 顶部标题栏 + 底部状态栏。
//!
//! 中文渲染加固：按候选顺序探测 Windows 自带 CJK 字体（微软雅黑/黑体/等线/宋体），
//! 命中后注入 Proportional 与 Monospace 两个家族的索引 0（Latin 走其 fallback），
//! 缺字体时打印警告但 GUI 仍可用。
//!
//! 页面：主页启动 / 登录 / 加载器 / 内容库（Modrinth）/ 原版 / 设置。
//! 后台任务：tokio spawn_blocking + std mpsc 桥接，UI 线程零阻塞。

use crate::auth::Session;
use crate::manifest::{self, Manifest};
use eframe::egui;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

const LOGO_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), r"\brand\logo-512.png"));

const APP_NAME: &str = "mc-native-exp";
const APP_SUB: &str = "实验启动器 · 可能不稳定";

/* ── 后台任务桥 ─────────────────────────────────────────────── */

pub type JobResult = (String, Result<String, String>);

pub struct Tasks {
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    pub handle: tokio::runtime::Handle,
    pub client: reqwest::Client,
    pub data: PathBuf,
}

impl Tasks {
    fn run<F>(&mut self, desc: impl Into<String>, f: F)
    where
        F: FnOnce(&reqwest::Client, &Path) -> Result<String, String> + Send + 'static,
    {
        let tx = self.tx.clone();
        let desc = desc.into();
        let client = self.client.clone();
        let data = self.data.clone();
        self.handle.spawn_blocking(move || {
            let r = f(&client, &data);
            let _ = tx.send((desc, r));
        });
    }

    pub fn poll(&mut self) -> Vec<JobResult> {
        let mut done = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            done.push(r);
        }
        done
    }
}

/// 在 spawn_blocking 任务线程里驱动 async（线程自带 tokio 上下文）。
pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Handle::current().block_on(fut)
}

/* ── Modrinth 内容库 ───────────────────────────────────────── */

#[derive(Debug, Clone, Deserialize)]
pub struct MSearchHit {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub downloads: u64,
}

/// (显示名, modrinth project_type, 安装子目录, 需要 loaders 过滤)
pub const CONTENT_TYPES: &[(&str, &str, &str, bool)] = &[
    ("模组", "mod", "mods", true),
    ("材质包", "resourcepack", "resourcepacks", false),
    ("光影", "shader", "shaderpacks", false),
    ("地图", "datapack", "datapacks", false),
    ("插件", "plugin", "plugins", true),
];

pub async fn modrinth_search(
    client: &reqwest::Client,
    ptype: &str,
    query: &str,
    mc: &str,
) -> Result<Vec<MSearchHit>, String> {
    let mut facets = format!("[[\"project_type:{ptype}\"]]");
    if !mc.is_empty() {
        facets = format!("[[\"project_type:{ptype}\"],[\"versions:{mc}\"]]");
    }
    let url = format!(
        "https://api.modrinth.com/v2/search?limit=20&index=downloads&query={}&facets={}",
        urlencoding_lite(query),
        facets
    );
    let v: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Modrinth 搜索：{e}"))?
        .json()
        .await
        .map_err(|e| format!("{e}"))?;
    let mut out = Vec::new();
    if let Some(hits) = v.get("hits").and_then(|x| x.as_array()) {
        for h in hits {
            out.push(MSearchHit {
                slug: h.get("slug").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                title: h.get("title").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                description: h
                    .get("description")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string(),
                downloads: h.get("downloads").and_then(|x| x.as_u64()).unwrap_or(0),
            });
        }
    }
    Ok(out)
}

pub async fn modrinth_download(
    client: &reqwest::Client,
    ptype: &str,
    slug: &str,
    mc: &str,
    data: &Path,
) -> Result<String, String> {
    let mut url = format!("https://api.modrinth.com/v2/project/{slug}/version?limit=1");
    if !mc.is_empty() {
        url = format!("{url}&game_versions=%5B%22{mc}%22%5D");
    }
    if let Some((_, _, _, needs_loader)) = CONTENT_TYPES.iter().find(|(n, ..)| *n == ptype) {
        if *needs_loader {
            let loader = if ptype == "plugin" { "paper" } else { "fabric" };
            url = format!("{url}&loaders=%5B%22{loader}%22%5D");
        }
    }
    let versions: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("版本列表：{e}"))?
        .json()
        .await
        .map_err(|e| format!("{e}"))?;
    let file_url = versions
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v0| v0.get("files"))
        .and_then(|f| f.as_array())
        .and_then(|f| f.first())
        .and_then(|f| f.get("url"))
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("{slug} 没有 {mc} 兼容文件（换 MC 版本重试）"))?
        .to_string();
    let file_name = file_url.rsplit('/').next().unwrap_or("content.bin").to_string();
    let sub = CONTENT_TYPES
        .iter()
        .find(|(n, _, _, _)| *n == ptype)
        .map(|(_, _, dir, _)| *dir)
        .unwrap_or("mods");
    let dest = data.join("game").join(sub).join(&file_name);
    crate::fetch::download_batch(client, vec![(file_url, dest, String::new(), 0)], slug).await?;
    Ok(format!("已安装到 game\\{sub}\\{file_name}"))
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/* ── 字体 ──────────────────────────────────────────────────── */

/// Windows 自带 CJK 字体候选（按观感优先）。
const CJK_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",   // 微软雅黑
    r"C:\Windows\Fonts\msyhbd.ttc", // 微软雅黑 Bold
    r"C:\Windows\Fonts\Deng.ttf",   // 等线
    r"C:\Windows\Fonts\Dengb.ttf",
    r"C:\Windows\Fonts\simhei.ttf", // 黑体
    r"C:\Windows\Fonts\simsun.ttc", // 宋体
];

fn load_cjk_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut loaded = String::new();
    for cand in CJK_FONT_CANDIDATES {
        if let Ok(bytes) = std::fs::read(cand) {
            fonts.font_data.insert("cjk".into(), egui::FontData::from_owned(bytes).into());
            loaded = (*cand).to_string();
            break;
        }
    }
    if loaded.is_empty() {
        eprintln!("警告：未找到系统 CJK 字体，中文可能显示为方框");
        return;
    }
    // 注入两个家族的索引 0：所有文字优先用 CJK 字体（含其自带 Latin），
    // 默认字体排后作 emoji/补充字形 fallback。
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().insert(0, "cjk".into());
    }
    ctx.set_fonts(fonts);
    println!("CJK 字体：{loaded}");
}

fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let img = image::load_from_memory(LOGO_PNG).expect("logo png");
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    ctx.load_texture(
        "logo",
        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}

/* ── App 状态 ──────────────────────────────────────────────── */

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Home,
    Login,
    Loaders,
    Content,
    Vanilla,
    Settings,
}

pub struct McApp {
    tasks: Tasks,
    results: Vec<JobResult>,
    login_name: Option<String>,
    tab: Tab,
    tex_logo: egui::TextureHandle,
    loader_sel: usize,
    mc_input: String,
    lv_input: String,
    ctype_sel: usize,
    query: String,
    results_mc: String,
    search_hits: Option<Vec<MSearchHit>>,
    manifest: Option<Manifest>,
    mc_sel: Option<String>,
    mem_mb: u32,
    versions: Vec<String>,
    show_log: bool,
}

impl McApp {
    fn new(cc: &eframe::CreationContext<'_>, handle: tokio::runtime::Handle, data: PathBuf) -> Self {
        load_cjk_fonts(&cc.egui_ctx);
        let tex_logo = logo_texture(&cc.egui_ctx);

        let (tx, rx) = mpsc::channel();
        let session = std::fs::read_to_string(data.join("token.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Session>(&t).ok())
            .map(|s| s.player_name);
        let manifest = manifest::load(&data).ok();
        let versions = list_installed(&data);
        let mc_sel = manifest.as_ref().map(|m| m.latest.release.clone());
        let client = reqwest::Client::builder()
            .http1_only()
            .user_agent("mc-native-exp/0.1")
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("http client");

        Self {
            tasks: Tasks { tx, rx, handle, client, data },
            results: Vec::new(),
            login_name: session,
            tab: Tab::Home,
            tex_logo,
            loader_sel: 0,
            mc_input: "1.21.4".into(),
            lv_input: String::new(),
            ctype_sel: 0,
            query: String::new(),
            results_mc: "1.21.4".into(),
            search_hits: None,
            manifest,
            mc_sel,
            mem_mb: 4096,
            versions,
            show_log: true,
        }
    }

    fn refresh_versions(&mut self) {
        self.versions = list_installed(&self.tasks.data);
    }

    fn push_log(&mut self, s: impl Into<String>) {
        self.results.insert(0, (String::new(), Ok(s.into())));
        if self.results.len() > 80 {
            self.results.truncate(80);
        }
    }

    /* ── 页面 ── */

    fn ui_home(&mut self, ui: &mut egui::Ui) {
        crate::theme::sci_label(ui, "启动 LAUNCH", crate::theme::CYAN);
        if let Some(name) = self.login_name.clone() {
            ui.colored_label(egui::Color32::from_rgb(124, 232, 200), format!("正版会话：{name}"));
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(255, 200, 97),
                "⚠ 未登录正版账号——请先到「登录」页完成设备码认证",
            );
        }
        ui.add_space(6.0);
        if self.versions.is_empty() {
            ui.weak("还没有已安装版本——去「加载器」或「原版」页安装一个。");
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for v in self.versions.clone() {
                    let v_job = v.clone();
                    ui.horizontal(|ui| {
                        if crate::theme::sci_button(ui, &format!("▶  {v}"), crate::theme::CYAN).clicked()
                        {
                            let mem = self.mem_mb;
                            self.tasks.run(format!("游戏退出({v})"), move |client, data| {
                                block_on(crate::launch::run(client, data, &v_job, mem, 0))
                                    .map(|_: ()| -> String { "正常退出".to_string() })
                            });
                            self.push_log(format!("启动 {v} …（关闭游戏窗口后自动回收）"));
                        }
                        if ui.small_button("🗑").on_hover_text("删除该版本").clicked() {
                            let _ = std::fs::remove_dir_all(
                                self.tasks.data.join("versions").join(&v),
                            );
                            self.refresh_versions();
                        }
                    });
                }
            });
    }

    fn ui_login(&mut self, ui: &mut egui::Ui) {
        crate::theme::sci_label(ui, "微软正版账号 AUTH", crate::theme::MINT);
        if let Some(name) = self.login_name.clone() {
            ui.colored_label(egui::Color32::from_rgb(124, 232, 200), format!("已登录：{name}"));
            ui.label("token 存 data\\token.json，24h 内自动续期。");
        } else {
            ui.label("未登录。点下方按钮开始设备码流程（浏览器打开 microsoft.com/link 输码）。");
        }
        ui.add_space(6.0);
        if crate::theme::sci_button(ui, "🔑 登录 / 刷新会话", crate::theme::MINT).clicked() {
            self.tasks.run("登录", |client, data| {
                let s = block_on(crate::auth::login(client, data))?;
                Ok(s.player_name)
            });
        }
        ui.add_space(8.0);
        ui.weak("实验前提：必须有 Minecraft: Java Edition 所有权，无正版拒绝继续。");
    }

    fn ui_loaders(&mut self, ui: &mut egui::Ui) {
        crate::theme::sci_label(ui, "模组加载器 LOADERS", crate::theme::CYAN);
        ui.horizontal(|ui| {
            ui.label("加载器：");
            for (i, name) in crate::loaders::SUPPORTED.iter().enumerate() {
                if ui.selectable_label(self.loader_sel == i, *name).clicked() {
                    self.loader_sel = i;
                }
            }
        });
        egui::Grid::new("ld")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("MC 版本");
                ui.text_edit_singleline(&mut self.mc_input);
                ui.end_row();
                ui.label("加载器版本");
                ui.text_edit_singleline(&mut self.lv_input);
                ui.end_row();
            });
        ui.weak("（加载器版本留空 = 自动取该 MC 的最新）");
        ui.add_space(4.0);
        if crate::theme::sci_button(ui, "⚙ 安装", crate::theme::CYAN).clicked() {
            let loader = crate::loaders::SUPPORTED[self.loader_sel].to_string();
            let mc = self.mc_input.clone();
            let lv = self.lv_input.clone();
            self.tasks.run(format!("安装 {loader}"), move |client, data| {
                block_on(crate::loaders::install(client, data, &loader, &mc, opt(&lv).as_deref()))
                    .map(|id| format!("版本 id {id}"))
            });
        }
    }

    fn ui_content(&mut self, ui: &mut egui::Ui) {
        crate::theme::sci_label(ui, "内容库 CONTENT · Modrinth", crate::theme::VIOLET);
        ui.horizontal(|ui| {
            for (i, (name, ..)) in CONTENT_TYPES.iter().enumerate() {
                if ui.selectable_label(self.ctype_sel == i, *name).clicked() {
                    self.ctype_sel = i;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("搜索：");
            let resp = ui.text_edit_singleline(&mut self.query);
            let (pt, ..) = CONTENT_TYPES[self.ctype_sel];
            let mc = self.results_mc.clone();
            let search_clicked = crate::theme::sci_button(ui, "🔍 搜索", crate::theme::VIOLET).clicked()
                || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if search_clicked && !self.query.trim().is_empty() {
                let q = self.query.clone();
                let pt = pt.to_string();
                self.tasks.run(format!("搜索 {pt}"), move |client, _data| {
                    let hits = block_on(modrinth_search(client, &pt, &q, &mc))?;
                    Ok(hits
                        .iter()
                        .map(|h| {
                            format!(
                                "{}\t{}\t{}\t{}",
                                h.slug,
                                h.title,
                                h.description.replace(['\t', '\n'], " "),
                                h.downloads
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"))
                });
            }
        });
        ui.horizontal(|ui| {
            ui.label("MC 版本：");
            ui.text_edit_singleline(&mut self.results_mc);
        });
        ui.add_space(6.0);
        let hits = self.search_hits.clone().unwrap_or_default();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for h in &hits {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.strong(&h.title);
                            ui.label(egui::RichText::new(&h.description).small());
                            ui.label(
                                egui::RichText::new(format!("⬇ {}", h.downloads)).small().weak(),
                            );
                        });
                        if crate::theme::sci_button(ui, "📥 安装", crate::theme::MINT).clicked() {
                            let (pt, ..) = CONTENT_TYPES[self.ctype_sel];
                            let pt = pt.to_string();
                            let slug = h.slug.clone();
                            let mc = self.results_mc.clone();
                            self.tasks.run(format!("安装 {slug}"), move |client, data| {
                                block_on(modrinth_download(client, &pt, &slug, &mc, data))
                            });
                        }
                    });
                    crate::theme::hud_separator(ui);
                }
            });
    }

    fn ui_vanilla(&mut self, ui: &mut egui::Ui) {
        crate::theme::sci_label(ui, "原版下载 VANILLA", crate::theme::CYAN);
        ui.horizontal(|ui| {
            ui.label("MC 版本：");
            egui::ComboBox::from_id_source("mcv")
                .selected_text(self.mc_sel.as_deref().unwrap_or("先拉清单"))
                .show_ui(ui, |ui| {
                    if let Some(m) = &self.manifest {
                        for v in m.versions.iter().filter(|v| v.kind == "release").take(60) {
                            ui.selectable_value(&mut self.mc_sel, Some(v.id.clone()), &v.id);
                        }
                    }
                });
            if crate::theme::sci_button(ui, "🔄 拉取/刷新清单", crate::theme::VIOLET).clicked() {
                self.tasks.run("刷新清单", |client, data| {
                    block_on(crate::manifest::fetch(client, data))?;
                    Ok("全部版本已缓存".into())
                });
            }
        });
        ui.add_space(4.0);
        if crate::theme::sci_button(
            ui,
            "⛏ 下载全套（client + libraries + assets + natives）",
            crate::theme::CYAN,
        )
        .clicked()
        {
            if let Some(mc) = self.mc_sel.clone() {
                let mc_job = mc.clone();
                self.tasks.run(format!("下载 {mc}"), move |client, data| {
                    block_on(crate::fetch::fetch_version(client, data, Some(&mc_job), false))
                        .map(|_: ()| -> String { "全套就绪".to_string() })
                });
            }
        }
        ui.weak("纯官方 piston-meta / piston-data / libraries.minecraft.net / resources.download.minecraft.net，SHA1 校验 + 断点续传。");
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        crate::theme::sci_label(ui, "设置 SETTINGS", crate::theme::AMBER);
        ui.horizontal(|ui| {
            ui.label("游戏内存 MB：");
            ui.add(egui::DragValue::new(&mut self.mem_mb).speed(100).range(1024..=16384));
        });
        ui.label(format!("数据目录：{}", self.tasks.data.display()));
        ui.label("JVM：自动下载 Adoptium Temurin JRE 到 data\\jre\\（加载器安装与模组版启动需要）");
        ui.add_space(8.0);
        if crate::theme::sci_button(ui, "🧹 清理安装器缓存 downloads", crate::theme::AMBER).clicked()
        {
            let _ = std::fs::remove_dir_all(self.tasks.data.join("downloads"));
            self.push_log("downloads 已清理");
        }
        ui.add_space(10.0);
        ui.weak(format!(
            "{} v{} · 实验项目，可能不稳定 · 正式使用请选 Aurora Launcher",
            APP_NAME,
            env!("CARGO_PKG_VERSION")
        ));
    }
}

fn list_installed(data: &Path) -> Vec<String> {
    let dir = data.join("versions");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir()
                && p.join(format!("{}.json", e.file_name().to_string_lossy())).exists()
            {
                out.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    out.reverse();
    out
}

fn opt(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.trim().to_string())
}

/* ── 布局 ──────────────────────────────────────────────────── */

const NAV: &[(Tab, &str, &str)] = &[
    (Tab::Home, "🚀", "主页"),
    (Tab::Login, "🔑", "登录"),
    (Tab::Loaders, "🧩", "加载器"),
    (Tab::Content, "📦", "内容库"),
    (Tab::Vanilla, "⛏", "原版"),
    (Tab::Settings, "⚙", "设置"),
];

fn page_title(tab: Tab) -> &'static str {
    match tab {
        Tab::Home => "启动",
        Tab::Login => "正版账号",
        Tab::Loaders => "模组加载器",
        Tab::Content => "内容库 · Modrinth",
        Tab::Vanilla => "原版下载",
        Tab::Settings => "设置",
    }
}

impl eframe::App for McApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::theme::install(ctx);

        // 回收后台任务
        for (desc, res) in self.tasks.poll() {
            if let Ok(msg) = &res {
                if desc.starts_with("登录") {
                    self.login_name = Some(msg.clone());
                }
                if desc.starts_with("搜索 ") {
                    let hits: Vec<MSearchHit> = msg
                        .lines()
                        .filter_map(|l| {
                            let p: Vec<&str> = l.split('\u{0009}').collect();
                            (p.len() >= 4).then(|| MSearchHit {
                                slug: p[0].into(),
                                title: p[1].into(),
                                description: p[2].into(),
                                downloads: p[3].parse().unwrap_or(0),
                            })
                        })
                        .collect();
                    self.search_hits = Some(hits);
                }
                if desc.starts_with("刷新清单") {
                    self.manifest = manifest::load(&self.tasks.data).ok();
                    self.mc_sel = self.manifest.as_ref().map(|m| m.latest.release.clone());
                }
                if desc.starts_with("安装 ")
                    || desc.starts_with("下载 ")
                    || desc.starts_with("游戏退出")
                {
                    self.refresh_versions();
                }
            }
            self.results.insert(0, (desc, res));
            if self.results.len() > 80 {
                self.results.truncate(80);
            }
        }

        // ── 左侧导航栏 ──
        egui::SidePanel::left("sidebar")
            .exact_width(200.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(crate::theme::PANEL)
                    .inner_margin(egui::Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.add(egui::Image::new(&self.tex_logo).max_size(egui::vec2(96.0, 96.0)));
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(APP_NAME)
                            .strong()
                            .heading()
                            .color(crate::theme::TEXT),
                    );
                    ui.label(egui::RichText::new(APP_SUB).small().color(crate::theme::AMBER));
                });
                ui.add_space(10.0);
                crate::theme::hud_separator(ui);
                ui.add_space(8.0);

                for (t, icon, label) in NAV {
                    let sel = self.tab == *t;
                    let text = egui::RichText::new(format!("{icon}  {label}")).color(if sel {
                        crate::theme::CYAN
                    } else {
                        egui::Color32::from_rgb(190, 198, 214)
                    });
                    let resp =
                        ui.add_sized([ui.available_width(), 36.0], egui::SelectableLabel::new(sel, text));
                    if sel {
                        let rect = resp.rect;
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(rect.left() - 8.0, rect.top()),
                                egui::vec2(3.0, rect.height()),
                            ),
                            2.0,
                            crate::theme::CYAN,
                        );
                    }
                    if resp.clicked() {
                        self.tab = *t;
                    }
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .small()
                            .color(crate::theme::MUTED),
                    );
                    ui.label(
                        egui::RichText::new("⚠ 实验项目 · 可能不稳定")
                            .small()
                            .color(crate::theme::AMBER),
                    );
                });
            });

        // ── 顶部标题栏 ──
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::none()
                    .fill(crate::theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(16.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(page_title(self.tab))
                            .heading()
                            .color(crate::theme::TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match &self.login_name {
                            Some(name) => ui.colored_label(
                                egui::Color32::from_rgb(124, 232, 200),
                                format!("👤 {name}"),
                            ),
                            None => ui.colored_label(crate::theme::AMBER, "👤 未登录"),
                        };
                    });
                });
            });

        // ── 底部状态栏 ──
        egui::TopBottomPanel::bottom("log")
            .frame(
                egui::Frame::none()
                    .fill(crate::theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(12.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let arrow = if self.show_log { "▾" } else { "▸" };
                    if ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(format!("{arrow} 运行状态"))
                                    .small()
                                    .strong()
                                    .color(crate::theme::CYAN),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .clicked()
                    {
                        self.show_log = !self.show_log;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("已装版本 {}", self.versions.len()))
                                .small()
                                .color(crate::theme::MUTED),
                        );
                    });
                });
                if self.show_log {
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .stick_to_bottom(false)
                        .show(ui, |ui| {
                            for (desc, res) in &self.results {
                                match res {
                                    Ok(msg) if desc.is_empty() => ui
                                        .label(egui::RichText::new(msg).small().monospace()),
                                    Ok(msg) => ui.label(
                                        egui::RichText::new(format!("✅ {desc}：{msg}"))
                                            .small()
                                            .monospace()
                                            .color(egui::Color32::from_rgb(124, 232, 200)),
                                    ),
                                    Err(e) => ui.label(
                                        egui::RichText::new(format!("❌ {desc}：{e}"))
                                            .small()
                                            .monospace()
                                            .color(egui::Color32::from_rgb(240, 140, 140)),
                                    ),
                                };
                            }
                        });
                }
            });

        // ── 主内容区 ──
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Home => self.ui_home(ui),
            Tab::Login => self.ui_login(ui),
            Tab::Loaders => self.ui_loaders(ui),
            Tab::Content => self.ui_content(ui),
            Tab::Vanilla => self.ui_vanilla(ui),
            Tab::Settings => self.ui_settings(ui),
        });
    }
}

/* ── 入口 ──────────────────────────────────────────────────── */

pub fn run_gui(handle: tokio::runtime::Handle, data: PathBuf) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{APP_NAME} · {APP_SUB}"))
            .with_inner_size([1080.0, 700.0])
            .with_min_inner_size([920.0, 600.0])
            .with_icon(egui_icon()),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(McApp::new(cc, handle, data)) as Box<dyn eframe::App>)),
    )
    .map_err(|e| format!("GUI：{e}"))
}

fn egui_icon() -> egui::IconData {
    let img = image::load_from_memory(LOGO_PNG).expect("logo png");
    let rgba = img.to_rgba8();
    egui::IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
}
