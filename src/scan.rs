//! 静态特性扫描：解析 client jar 里全部 class，统计「Rust 解释器/转译器
//! 必须具备的能力」分布——native 方法（JNI）、invokedynamic（LambdaMetafactory）、
//! 类文件版本等。数据落 data\scan-<version>.json，REPORT.md 汇总用。

use crate::classparse::{parse, ClassFile, FeatureStats};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub version: String,
    pub jar: String,
    pub classes_total: usize,
    pub classes_parsed_ok: usize,
    pub classes_parse_failed: usize,
    pub parse_errors_sample: Vec<String>,
    pub features: FeatureStats,
    /// 依赖的 java/* 之外的外部类前缀统计（LWJGL、guava 等），前 20。
    pub external_packages: BTreeMap<String, usize>,
    /// 依赖的 JDK 类（java/ 前缀）数量。
    pub jdk_class_refs: usize,
}

impl ScanResult {
    fn scan_jar(version: &str, jar_path: &Path) -> Result<ScanResult, String> {
        let f = std::fs::File::open(jar_path).map_err(|e| format!("打开 jar 失败：{e}"))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| format!("zip 打开失败：{e}"))?;
        let mut res = ScanResult {
            version: version.to_string(),
            jar: jar_path.display().to_string(),
            classes_total: 0,
            classes_parsed_ok: 0,
            classes_parse_failed: 0,
            parse_errors_sample: Vec::new(),
            features: FeatureStats::default(),
            external_packages: BTreeMap::new(),
            jdk_class_refs: 0,
        };
        // 类引用闭包：所有常量池里的 Class 引用（跨包统计）。
        let mut ext_refs: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut jdk_refs = 0usize;

        for i in 0..z.len() {
            let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().to_string();
            if !name.ends_with(".class") {
                continue;
            }
            res.classes_total += 1;
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("读 {name} 失败：{e}"))?;
            match parse(&bytes) {
                Ok(cf) => {
                    res.classes_parsed_ok += 1;
                    res.features.merge(&cf.scan_features());
                    collect_refs(&cf, &mut ext_refs, &mut jdk_refs);
                }
                Err(e) => {
                    res.classes_parse_failed += 1;
                    if res.parse_errors_sample.len() < 10 {
                        res.parse_errors_sample.push(format!("{name}: {e}"));
                    }
                }
            }
        }
        // 外部包前缀聚合（java/ 除外），取包前三段。
        for (name, n) in ext_refs {
            if name.starts_with("java/") || name.starts_with("javax/") {
                continue;
            }
            let key: String = {
                let segs: Vec<&str> = name.split('/').take(3).collect();
                segs.join("/")
            };
            *res.external_packages.entry(key).or_insert(0) += n;
        }
        res.jdk_class_refs = jdk_refs;
        // external_packages 只留前 20（按值）。
        let mut v: Vec<(String, usize)> = res.external_packages.clone().into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.truncate(20);
        res.external_packages = v.into_iter().collect();
        Ok(res)
    }
}

/// 从常量池收集本类引用到的全部类名。
fn collect_refs(
    cf: &ClassFile,
    ext: &mut std::collections::HashMap<String, usize>,
    jdk: &mut usize,
) {
    fn add(name: &str, ext: &mut std::collections::HashMap<String, usize>, jdk: &mut usize, depth: usize) {
        if name.is_empty() || depth > 3 {
            return;
        }
        if let Some(inner) = name.strip_prefix('[') {
            let inner = inner.trim_start_matches('L').trim_end_matches(';');
            add(inner, ext, jdk, depth + 1);
            return;
        }
        if name.starts_with("java/") || name.starts_with("javax/") {
            *jdk += 1;
        } else {
            *ext.entry(name.to_string()).or_insert(0) += 1;
        }
    }
    for info in cf.cp.iter().flatten() {
        if let crate::classparse::CpInfo::Class { name_index } = info {
            if let Some(n) = cf.cp_utf8(*name_index) {
                add(&n, ext, jdk, 0);
            }
        }
    }
}

pub fn run(data: &Path, version: Option<&str>, all: bool) -> Result<(), String> {
    let out_dir = data.join("scans");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{e}"))?;

    if all {
        let jars = data.join("jars");
        let mut files: Vec<PathBuf> = list_jars(&jars);
        files.sort();
        if files.is_empty() {
            return Err("data\\jars 里没有 client jar——先 fetch-all-clients".into());
        }
        println!("扫描 {} 个 jar……", files.len());
        for (idx, p) in files.iter().enumerate() {
            let v = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            print!("[{}/{}] {} … ", idx + 1, files.len(), v);
            match ScanResult::scan_jar(&v, p) {
                Ok(r) => {
                    println!(
                        "classes {}/{} ok, native {}, indy {}",
                        r.classes_parsed_ok, r.classes_total, r.features.native_methods, r.features.invokedynamic_sites
                    );
                    let out = out_dir.join(format!("scan-{v}.json"));
                    write_json(&out, &r)?;
                }
                Err(e) => println!("失败：{e}"),
            }
        }
        println!("全部扫描结果在 {}\\", out_dir.display());
        return Ok(());
    }

    let v = match version {
        Some(v) => v.to_string(),
        None => {
            // 取 versions\ 下已 fetch 的第一个，或 jars\ 里第一个
            let jars = data.join("jars");
            if let Some(p) = list_jars(&jars).first() {
                p.file_stem().unwrap_or_default().to_string_lossy().to_string()
            } else {
                return Err("未指定版本且没有已下载的 jar——先 fetch 或 fetch-all-clients".into());
            }
        }
    };
    let jar = crate::fetch::client_jar_path(data, &v)
        .ok_or_else(|| format!("版本 {v} 的 client jar 不存在——先 fetch {v} --client-only"))?;
    println!("扫描 {}（{}）……", v, jar.display());
    let r = ScanResult::scan_jar(&v, &jar)?;
    println!(
        "解析 {}/{} 个 class；native 方法 {}；invokedynamic 站点 {}；最高 class 版本 major {}",
        r.classes_parsed_ok, r.classes_total, r.features.native_methods, r.features.invokedynamic_sites, r.features.max_class_major
    );
    if !r.parse_errors_sample.is_empty() {
        println!("解析失败样例：");
        for s in &r.parse_errors_sample {
            println!("  {s}");
        }
    }
    let out = out_dir.join(format!("scan-{v}.json"));
    write_json(&out, &r)?;
    println!("结果已写入 {}", out.display());
    Ok(())
}



fn list_jars(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()).map(|x| x == "jar").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}

fn write_json(path: &Path, v: &impl serde::Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?;
    crate::manifest::write_atomic(path, &bytes)
}
