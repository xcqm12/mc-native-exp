//! 字节码解释器原型：不依赖任何 JVM，在 Rust 里解释执行官方 client jar。
//!
//! 目标不是把游戏跑起来（那需要完整 JVM + LWJGL + 图形栈），而是量化「差在哪」：
//! - 从版本 JSON 的 main_class 开始解释，统计能推进多少指令/方法/类；
//! - 精确记录撞墙点：缺 JDK 类 / 缺 libraries 依赖 / native 方法（JNI）/
//!   invokedynamic（LambdaMetafactory）/ 未实现 opcode；
//! - 输出依赖闭包规模（jar 内 vs JDK vs 第三方库）。
//!
//! 极小 JDK shim（System.out.println / String 方法等）只为多走几步并计数——
//! shim 调用量本身就是「需要模拟的 JDK 表面积」的实验数据。

use crate::classparse::{parse, ClassFile, CpInfo, Method, ACC_NATIVE, ACC_STATIC};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::Path;
use std::rc::Rc;

/// 指令预算：防失控（游戏本体千万级指令，预算内看推进趋势即可）。
const BUDGET: u64 = 200_000;
const MAX_DEPTH: usize = 48;

#[derive(Debug, Clone)]
enum Value {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Null,
    Str(Rc<String>),
    Obj(Rc<RefCell<HeapObj>>),
    Array(Rc<RefCell<Vec<Value>>>),
    ClassRef(String),
    /// long/double 第二槽的占位（调用参数对齐用）。
    Placeholder,
}

#[derive(Debug)]
struct HeapObj {
    class: String,
    fields: HashMap<String, Value>,
}

impl Value {
    fn as_i(&self) -> i64 {
        match self {
            Value::Int(v) => *v as i64,
            Value::Long(v) => *v,
            Value::Float(v) => *v as i64,
            Value::Double(v) => *v as i64,
            Value::Str(s) => s.len() as i64,
            Value::Array(a) => a.borrow().len() as i64,
            _ => 0,
        }
    }
    fn as_f(&self) -> f64 {
        match self {
            Value::Int(v) => *v as f64,
            Value::Long(v) => *v as f64,
            Value::Float(v) => *v as f64,
            Value::Double(v) => *v,
            _ => 0.0,
        }
    }
    fn truthy_int(&self) -> i32 {
        match self {
            Value::Int(v) => *v,
            _ => 0,
        }
    }
    fn describe(&self) -> String {
        match self {
            Value::Int(v) => format!("int {v}"),
            Value::Long(v) => format!("long {v}"),
            Value::Float(v) => format!("float {v}"),
            Value::Double(v) => format!("double {v}"),
            Value::Null => "null".into(),
            Value::Str(s) => format!("\"{}\"", s),
            Value::Obj(o) => format!("{}@obj", o.borrow().class.replace('/', ".")),
            Value::Array(_) => "array".into(),
            Value::ClassRef(c) => format!("class {}", c.replace('/', ".")),
            Value::Placeholder => "<slot>".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct Wall {
    kind: String,
    detail: String,
    chain: Vec<String>,
}

#[derive(Default)]
struct Stats {
    instructions: u64,
    methods_run: u64,
    classes_loaded: u64,
    shim_calls: u64,
    stdout_lines: Vec<String>,
    walls: Vec<Wall>,
    /// 每类墙的计数（report 汇总用）。
    wall_kinds: HashMap<String, usize>,
}

struct Interp<'s> {
    store: &'s RefCell<ClassStore>,
    stats: Stats,
    statics: HashMap<(String, String), Value>,
    initialized: HashSet<String>,
    depth: usize,
}

#[derive(Debug)]
enum Stop {
    /// 撞墙：详情已记入 stats.walls。
    Wall,
    /// 找不到方法。
    NoSuchMethod(String),
    /// 被预算/深度限制截断（非失败，是推进数据）。
    Truncated(&'static str),
    /// 正常返回值。
    Return(Value),
}

#[derive(Clone)]
struct Frame {
    cf: Rc<ClassFile>,
    method: String,
    /// 该方法 Code 属性的字节码（invoke 时克隆）。
    code: Vec<u8>,
    pc: usize,
    locals: Vec<Value>,
    stack: Vec<Value>,
}

impl Frame {
    fn u8(&mut self) -> u8 {
        let v = self.code.get(self.pc).copied().unwrap_or(0);
        self.pc += 1;
        v
    }
    fn s8(&mut self) -> i8 {
        self.u8() as i8
    }
    fn u16(&mut self) -> u16 {
        let a = self.u8() as u16;
        let b = self.u8() as u16;
        (a << 8) | b
    }
    fn i16(&mut self) -> i16 {
        self.u16() as i16
    }
    fn i32(&mut self) -> i32 {
        let a = self.u16() as u16;
        let b = self.u16() as u16;
        (((a as u32) << 16) | b as u32) as i32
    }
}

/// 从 jar 建立的类加载器。
struct ClassStore {
    map: HashMap<String, usize>,
    data: Vec<Vec<u8>>,
    cache: HashMap<String, Rc<ClassFile>>,
    loaded: u64,
}

impl ClassStore {
    fn from_jar(path: &Path) -> Result<ClassStore, String> {
        let f = std::fs::File::open(path).map_err(|e| format!("打开 jar：{e}"))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| format!("zip：{e}"))?;
        let mut map = HashMap::new();
        let mut data = Vec::new();
        for i in 0..z.len() {
            let mut e = z.by_index(i).map_err(|e| e.to_string())?;
            if e.name().ends_with(".class") {
                let mut buf = Vec::with_capacity(e.size() as usize);
                e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                let cname = e.name().trim_end_matches(".class").to_string();
                map.insert(cname, data.len());
                data.push(buf);
            }
        }
        Ok(ClassStore { map, data, cache: HashMap::new(), loaded: 0 })
    }

    fn load(&mut self, name: &str) -> Result<Option<Rc<ClassFile>>, String> {
        if let Some(c) = self.cache.get(name) {
            return Ok(Some(c.clone()));
        }
        let Some(&idx) = self.map.get(name) else {
            return Ok(None);
        };
        let cf = Rc::new(parse(&self.data[idx]).map_err(|e| format!("解析 {name}：{e}"))?);
        self.cache.insert(name.to_string(), cf.clone());
        self.loaded += 1;
        Ok(Some(cf))
    }

    fn has(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }
}

pub fn run(data: &Path, version: Option<&str>) -> Result<(), String> {
    let v = version
        .map(|s| s.to_string())
        .ok_or_else(|| "请指定版本，如 `interp 1.21.4`".to_string())?;
    let jar = crate::fetch::client_jar_path(data, &v)
        .ok_or_else(|| format!("版本 {v} 的 client jar 不存在——先 `fetch {v} --client-only`"))?;

    println!("解释器原型：{}（{}）", v, jar.display());
    println!("指令预算：{BUDGET}；调用深度上限：{MAX_DEPTH}");

    let store = ClassStore::from_jar(&jar)?;
    println!("jar 内 class 条目：{} 个", store.map.len());

    let main_class =
        main_class_of(data, &v).unwrap_or_else(|| "net/minecraft/client/main/Main".into());
    println!("入口：{}#main([Ljava/lang/String;)V", main_class.replace('/', "."));

    let store_cell = RefCell::new(store);
    let mut it = Interp {
        store: &store_cell,
        stats: Stats::default(),
        statics: HashMap::new(),
        initialized: HashSet::new(),
        depth: 0,
    };

    let args = Value::Array(Rc::new(RefCell::new(Vec::new())));
    let outcome = it.call_static(&main_class, "main", "([Ljava/lang/String;)V", vec![args]);

    let closure = closure_stats(&mut store_cell.borrow_mut(), &main_class);

    println!();
    println!("── 解释器推进 ──");
    println!(
        "执行指令 {}；调用方法 {}；加载类 {}；JDK shim 调用 {}；stdout 行 {}",
        it.stats.instructions, it.stats.methods_run, it.stats.classes_loaded,
        it.stats.shim_calls, it.stats.stdout_lines.len()
    );
    if !it.stats.stdout_lines.is_empty() {
        for l in it.stats.stdout_lines.iter().take(5) {
            println!("  [游戏 stdout] {l}");
        }
    }
    match outcome {
        Ok(Some(val)) => println!("入口返回：{}", val.describe()),
        Ok(None) => println!("入口返回 void"),
        Err(Stop::Wall) => println!("入口撞墙停止（见下）"),
        Err(Stop::NoSuchMethod(m)) => println!("入口方法缺失：{m}"),
        Err(Stop::Return(_)) => {}

        Err(Stop::Truncated(why)) => println!("入口被截断：{why}"),
    }
    if let Some(w) = it.stats.walls.first() {
        println!();
        println!("── 第一堵墙 ──");
        println!("类型：{}", w.kind);
        println!("详情：{}", w.detail);
        for (i, line) in w.chain.iter().enumerate() {
            println!("  {}↳ {}", "  ".repeat(i), line);
        }
    } else {
        println!("（预算内未撞墙——提升 BUDGET 可继续推进）");
    }
    println!();
    println!("── 墙种类分布（本预算内）──");
    let mut kinds: Vec<(&String, &usize)> = it.stats.wall_kinds.iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in kinds.iter().take(10) {
        println!("  {k}: {n}");
    }

    println!();
    println!("── 依赖闭包（入口类可达）──");
    println!(
        "jar 内 {} 个；JDK {} 个类名引用；jar 外第三方 {} 处",
        closure.in_jar, closure.jdk, closure.external
    );

    #[derive(Serialize)]
    struct Out {
        version: String,
        main_class: String,
        instructions: u64,
        methods_run: u64,
        classes_loaded: u64,
        shim_calls: u64,
        stdout_lines: Vec<String>,
        first_wall: Option<Wall>,
        wall_kinds: HashMap<String, usize>,
        closure_in_jar: usize,
        closure_jdk: usize,
        closure_external: usize,
    }
    let out = Out {
        version: v.clone(),
        main_class: main_class.clone(),
        instructions: it.stats.instructions,
        methods_run: it.stats.methods_run,
        classes_loaded: it.stats.classes_loaded,
        shim_calls: it.stats.shim_calls,
        stdout_lines: it.stats.stdout_lines.clone(),
        first_wall: it.stats.walls.first().cloned(),
        wall_kinds: it.stats.wall_kinds.clone(),
        closure_in_jar: closure.in_jar,
        closure_jdk: closure.jdk,
        closure_external: closure.external,
    };
    let out_path = data.join(format!("interp-{v}.json"));
    let bytes = serde_json::to_vec_pretty(&out).map_err(|e| e.to_string())?;
    crate::manifest::write_atomic(&out_path, &bytes)?;
    println!("数据已写入 {}", out_path.display());
    Ok(())
}

fn main_class_of(data: &Path, v: &str) -> Option<String> {
    let p = data.join("versions").join(v).join(format!("{v}.json"));
    let text = std::fs::read_to_string(p).ok()?;
    let j: serde_json::Value = serde_json::from_str(&text).ok()?;
    j.get("mainClass").and_then(|x| x.as_str()).map(|s| s.replace('.', "/"))
}

#[derive(Default)]
struct Closure {
    in_jar: usize,
    jdk: usize,
    external: usize,
}

/// 从入口类 BFS 引用闭包（jar 边界内），量化依赖规模。
fn closure_stats(store: &mut ClassStore, entry: &str) -> Closure {
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut c = Closure::default();
    queue.push_back(entry.to_string());
    seen.insert(entry.to_string());
    c.in_jar += 1;
    while let Some(name) = queue.pop_front() {
        let Ok(Some(cf)) = store.load(&name) else { continue };
        for info in cf.cp.iter().flatten() {
            if let CpInfo::Class { name_index } = info {
                if let Some(target) = cf.cp_utf8(*name_index) {
                    let t = target.trim_start_matches('[');
                    let t = t.trim_start_matches('L').trim_end_matches(';');
                    if t.is_empty() || t.starts_with('[') {
                        continue;
                    }
                    if t.starts_with("java/") || t.starts_with("javax/") || t.starts_with("sun/") || t.starts_with("jdk/") {
                        continue;
                    }
                    if store.has(t) {
                        if seen.insert(t.to_string()) {
                            c.in_jar += 1;
                            queue.push_back(t.to_string());
                        }
                    } else {
                        c.external += 1;
                    }
                }
            }
        }
    }
    // JDK 引用面：全 jar 引用过的 JDK 类名集合（一次全量统计）。
    let mut jdk_set = HashSet::new();
    for name in store.map.keys().cloned().collect::<Vec<_>>() {
        if let Ok(Some(cf)) = store.load(&name) {
            for info in cf.cp.iter().flatten() {
                if let CpInfo::Class { name_index } = info {
                    if let Some(t) = cf.cp_utf8(*name_index) {
                        let k = t.as_str();
                        if k.starts_with("java/") || k.starts_with("javax/") || k.starts_with("sun/") || k.starts_with("jdk/") {
                            jdk_set.insert(k.to_string());
                        }
                    }
                }
            }
        }
        c.jdk = jdk_set.len();
    }
    c
}

/* ── 解释器核心 ─────────────────────────────────────────────── */

impl<'s> Interp<'s> {
    fn wall(&mut self, kind: &str, detail: String, frames: &[Frame]) -> Stop {
        *self.stats.wall_kinds.entry(kind.to_string()).or_insert(0) += 1;
        if self.stats.walls.len() < 64 {
            self.stats.walls.push(Wall {
                kind: kind.into(),
                detail,
                chain: frames.iter().rev().take(8).map(|f| format!("{}#{} : pc{}", f.cf.this_class.replace('/', "."), f.method, f.pc)).collect(),
            });
        }
        Stop::Wall
    }

    /// 确保类已初始化（<clinit>）；返回 Err 表示初始化撞墙。
    fn ensure_init(&mut self, class: &str, caller: &Frame) -> Result<(), Stop> {
        if self.initialized.contains(class) || !self.store.borrow().has(class) {
            return Ok(());
        }
        self.initialized.insert(class.to_string());
        let cf = match self.store.borrow_mut().load(class) {
            Ok(Some(cf)) => cf,
            _ => return Ok(()),
        };
        if cf.find_method("<clinit>", "()V").is_some() {
            if self.depth >= MAX_DEPTH {
                return Err(Stop::Truncated("max-depth"));
            }
            self.depth += 1;
            let r = self.call_static(class, "<clinit>", "()V", vec![]);
            self.depth -= 1;
            r?;
        }
        let _ = caller;
        Ok(())
    }

    fn call_static(
        &mut self,
        class: &str,
        method: &str,
        desc: &str,
        args: Vec<Value>,
    ) -> Result<Option<Value>, Stop> {
        let cf = match self.store.borrow_mut().load(class) {
            Ok(Some(cf)) => cf,
            Ok(None) => {
                if let Some(r) = self.jdk_static(class, method, desc, &args) {
                    return Ok(r);
                }
                return Err(self.wall(
                    "MissingClass",
                    format!("静态调用目标类不在 client jar：{class}.{method}{desc}（JDK 或 libraries 提供）"),
                    &[],
                ));
            }
            Err(e) => return Err(self.wall("ParseFail", e, &[])),
        };
        let Some(m) = cf.find_method(method, desc).cloned() else {
            return Err(Stop::NoSuchMethod(format!("{class}.{method}{desc}")));
        };
        self.invoke(cf, &m, args)
    }

    fn call_virtual(
        &mut self,
        receiver: &Value,
        class_hint: &str,
        method: &str,
        desc: &str,
        args: Vec<Value>,
    ) -> Result<Option<Value>, Stop> {
        let owner: String = match receiver {
            Value::Str(_) => "java/lang/String".to_string(),
            Value::Obj(o) => o.borrow().class.clone(),
            _ => class_hint.to_string(),
        };
        if let Some(r) = self.jdk_virtual(receiver, owner.as_str(), method, desc, args.as_slice()) {
            return Ok(r);
        }
        let cf = match self.store.borrow_mut().load(&owner) {
            Ok(Some(cf)) => cf,
            Ok(None) => {
                return Err(self.wall(
                    "MissingClass",
                    format!("虚调用目标类不在 jar：{owner}.{method}{desc}（JDK/libraries 提供）"),
                    &[],
                ))
            }
            Err(e) => return Err(self.wall("ParseFail", e, &[])),
        };
        let Some(m) = cf.find_method(method, desc).cloned() else {
            if let Some(sup) = cf.super_class.clone() {
                if self.store.borrow().has(&sup) {
                    return self.call_virtual(receiver, &sup, method, desc, args);
                }
            }
            return Err(Stop::NoSuchMethod(format!("{owner}.{method}{desc}")));
        };
        self.invoke(cf, &m, args)
    }

    fn invoke(&mut self, cf: Rc<ClassFile>, m: &Method, args: Vec<Value>) -> Result<Option<Value>, Stop> {
        if m.access & ACC_NATIVE != 0 {
            return Err(self.wall(
                "NativeMethod",
                format!("{}#{}{} 是 native 方法——JNI 实现（LWJGL/OpenGL/平台库），Rust 侧必须提供等价原生实现", cf.this_class.replace('/', "."), m.name, m.descriptor),
                &[],
            ));
        }
        if m.access & crate::classparse::ACC_ABSTRACT != 0 {
            return Err(self.wall("AbstractMethod", format!("{}#{}{} 抽象方法", cf.this_class, m.name, m.descriptor), &[]));
        }
        let Some(code) = m.code.clone() else { return Ok(None) };
        if self.depth >= MAX_DEPTH {
            return Err(Stop::Truncated("max-depth"));
        }
        self.depth += 1;
        self.stats.methods_run += 1;
        let nlocals = locals_needed(&m.descriptor, m.access & ACC_STATIC == 0).max(m.max_locals as usize);
        let mut fr = Frame {
            cf: cf.clone(),
            method: m.name.clone(),
            pc: 0,
            locals: vec![Value::Null; nlocals],
            stack: Vec::with_capacity(m.max_stack as usize + 8),
            code,
        };
        // 实参装填 locals：实例方法 slot0=this；long/double 占两槽（第二槽占位）。
        let mut slot = 0usize;
        if m.access & ACC_STATIC == 0 {
            fr.locals[0] = args.first().cloned().unwrap_or(Value::Null);
            slot = 1;
        }
        let mut ai = if m.access & ACC_STATIC == 0 { 1 } else { 0 };
        for w in param_widths(&m.descriptor) {
            if slot >= fr.locals.len() {
                break;
            }
            fr.locals[slot] = args.get(ai).cloned().unwrap_or(Value::Null);
            ai += 1;
            if w == 2 {
                slot += 2;
            } else {
                slot += 1;
            }
        }
        let r = self.exec(&mut fr);
        self.depth -= 1;
        r
    }

    fn exec(&mut self, fr: &mut Frame) -> Result<Option<Value>, Stop> {
        loop {
            self.stats.instructions += 1;
            if self.stats.instructions > BUDGET {
                return Err(Stop::Truncated("budget"));
            }
            let op = match fr.code.get(fr.pc) {
                Some(&b) => b,
                None => return Ok(None),
            };
            fr.pc += 1;
            match op {
                /* ── 常量 ── */
                0x00 => {}
                0x01 => fr.stack.push(Value::Null),
                0x02..=0x08 => fr.stack.push(Value::Int(op as i32 - 0x03)),
                0x09 | 0x0A => fr.stack.push(Value::Long(0)),
                0x0B | 0x0C => fr.stack.push(Value::Long(1)),
                0x0D => fr.stack.push(Value::Float(0.0)),
                0x0E => fr.stack.push(Value::Float(1.0)),
                0x0F => fr.stack.push(Value::Double(0.0)),
                0x10 => {
                    let b = fr.s8() as i32;
                    fr.stack.push(Value::Int(b));
                }
                0x11 => {
                    let s = fr.i16() as i32;
                    fr.stack.push(Value::Int(s));
                }
                0x12 | 0x13 | 0x14 => {
                    let idx = if op == 0x12 { fr.u8() as u16 } else { fr.u16() };
                    self.ldc(fr, idx)?;
                }
                /* ── 加载 ── */
                0x15 | 0x16 | 0x17 | 0x18 | 0x19 => {
                    let idx = fr.u8() as usize;
                    fr.stack.push(fr.locals.get(idx).cloned().unwrap_or(Value::Null));
                }
                0x1A..=0x1D => fr.stack.push(fr.locals[(op - 0x1A) as usize].clone()),
                0x1E..=0x21 => fr.stack.push(fr.locals[(op - 0x1E) as usize * 2].clone()),
                0x22..=0x25 => fr.stack.push(fr.locals[(op - 0x22) as usize].clone()),
                0x26..=0x29 => fr.stack.push(fr.locals[(op - 0x26) as usize * 2].clone()),
                0x2A..=0x2D => fr.stack.push(fr.locals[(op - 0x2A) as usize].clone()),
                /* ── 存储 ── */
                0x36 | 0x37 | 0x38 | 0x39 | 0x3A => {
                    let idx = fr.u8() as usize;
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    if idx < fr.locals.len() {
                        fr.locals[idx] = v;
                    }
                }
                0x3B..=0x3E => {
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    fr.locals[(op - 0x3B) as usize] = v;
                }
                0x3F..=0x42 => {
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    fr.locals[(op - 0x3F) as usize * 2] = v;
                }
                0x43..=0x46 => {
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    fr.locals[(op - 0x43) as usize] = v;
                }
                0x47..=0x4A => {
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    fr.locals[(op - 0x47) as usize * 2] = v;
                }
                0x4B..=0x4E => {
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    fr.locals[(op - 0x4B) as usize] = v;
                }
                /* ── 栈操作 ── */
                0x57 => {
                    fr.stack.pop();
                }
                0x58 => {
                    // pop2：两个 slot（一个 long/double 或两个单项）
                    let n = fr.stack.len();
                    if n >= 2 && !matches!(fr.stack[n - 1], Value::Long(_) | Value::Double(_)) {
                        fr.stack.pop();
                    }
                    fr.stack.pop();
                }
                0x59 => {
                    let v = fr.stack.last().cloned().unwrap_or(Value::Null);
                    fr.stack.push(v);
                }
                0x5A => {
                    let n = fr.stack.len();
                    if n >= 2 {
                        let v = fr.stack.remove(n - 1);
                        let w = fr.stack.remove(n - 2);
                        fr.stack.insert(n - 2, v);
                        fr.stack.insert(n - 2, w);
                    }
                }
                0x5C => {
                    let n = fr.stack.len();
                    if n >= 2 {
                        let v = fr.stack[n - 1].clone();
                        let w = fr.stack[n - 2].clone();
                        if matches!(w, Value::Long(_) | Value::Double(_)) {
                            fr.stack.push(v);
                        } else {
                            fr.stack.push(w);
                            fr.stack.push(v);
                        }
                    }
                }
                0x5F => {
                    let n = fr.stack.len();
                    if n >= 2 {
                        fr.stack.swap(n - 1, n - 2);
                    }
                }
                /* ── 算术（int/long 精确；float/double 用 f64 承载） ── */
                0x60..=0x73 => self.arith(fr, op),
                0x74 => { let v = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0); fr.stack.push(Value::Int((-(v as i64)) as i32)); }
                0x75 => { let v = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0); fr.stack.push(Value::Long(-v)); }
                0x76 => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Float(-v as f32)); }
                0x77 => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Double(-v)); }
                0x78 => { let (a, b) = self.pop2_i(fr); fr.stack.push(Value::Int(((a as u32) << (b as u32 & 31)) as i32)); }
                0x79 => { let (a, b) = self.pop2_i(fr); fr.stack.push(Value::Int((a >> (b as u32 & 31)) as i32)); }
                0x7A => { let (a, b) = self.pop2_i(fr); fr.stack.push(Value::Int(((a as u32) >> (b as u32 & 31)) as i32)); }
                0x7B => { let (a, b) = self.pop2_l(fr); fr.stack.push(Value::Long(((a as u64) << (b as u64 & 63)) as i64)); }
                0x7C => { let (a, b) = self.pop2_l(fr); fr.stack.push(Value::Long(a >> (b as u64 & 63))); }
                0x7D => { let (a, b) = self.pop2_l(fr); fr.stack.push(Value::Long(((a as u64) >> (b as u64 & 63)) as i64)); }
                0x7E => { let (a, b) = self.pop2_ii(fr); fr.stack.push(Value::Int(a & b)); }
                0x7F => { let (a, b) = self.pop2_l(fr); fr.stack.push(Value::Long(a & b)); }
                0x80 => { let (a, b) = self.pop2_ii(fr); fr.stack.push(Value::Int(a | b)); }
                0x81 => { let (a, b) = self.pop2_l(fr); fr.stack.push(Value::Long(a | b)); }
                0x82 => { let (a, b) = self.pop2_ii(fr); fr.stack.push(Value::Int(a ^ b)); }
                0x83 => { let (a, b) = self.pop2_l(fr); fr.stack.push(Value::Long(a ^ b)); }
                /* ── 转换 ── */
                0x85 => { let v = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0); fr.stack.push(Value::Long(v)); }
                0x86 => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Float(v as f32)); }
                0x87 => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Double(v)); }
                0x88 => { let v = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0); fr.stack.push(Value::Int(v as i32)); }
                0x89 => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Float(v as f32)); }
                0x8A => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Double(v)); }
                0x8B => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Int(v as i32)); }
                0x8C => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Long(v as i64)); }
                0x8D => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Double(v)); }
                0x8E => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Int(v as i32)); }
                0x8F => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Long(v as i64)); }
                0x90 => { let v = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0); fr.stack.push(Value::Float(v as f32)); }
                0x91 => { let v = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0); fr.stack.push(Value::Int(v as i8 as i32)); }
                0x92 => { let v = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0); fr.stack.push(Value::Int(v as u16 as i32)); }
                0x93 => { let v = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0); fr.stack.push(Value::Int(v as i16 as i32)); }
                /* ── 比较 ── */
                0x94 => {
                    let b = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
                    let a = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
                    let r = if a < b { -1 } else if a > b { 1 } else { 0 };
                    fr.stack.push(Value::Int(r));
                }
                0x95 | 0x96 | 0x97 | 0x98 => {
                    let b = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0);
                    let a = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0);
                    let nan_bias = matches!(op, 0x96 | 0x98);
                    let r = if a.is_nan() || b.is_nan() {
                        if nan_bias { 1 } else { -1 }
                    } else if a < b {
                        -1
                    } else if a > b {
                        1
                    } else {
                        0
                    };
                    fr.stack.push(Value::Int(r));
                }
                /* ── 分支 ── */
                0x99..=0x9E => {
                    let v = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
                    let off = fr.i16();
                    let taken = match op {
                        0x99 => v == 0,
                        0x9A => v != 0,
                        0x9B => v < 0,
                        0x9C => v >= 0,
                        0x9D => v > 0,
                        _ => v <= 0,
                    };
                    if taken {
                        fr.pc = (fr.pc as i32 + off as i32 - 3 + 0) as usize; // off 相对分支指令起点
                    }
                }
                0x9F..=0xA4 => {
                    let b = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
                    let a = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
                    let off = fr.i16();
                    let taken = match op {
                        0x9F => a == b,
                        0xA0 => a != b,
                        0xA1 => a < b,
                        0xA2 => a >= b,
                        0xA3 => a > b,
                        _ => a <= b,
                    };
                    if taken {
                        fr.pc = (fr.pc as i32 + off as i32 - 3) as usize;
                    }
                }
                0xA5 | 0xA6 => {
                    let b = fr.stack.pop().unwrap_or(Value::Null);
                    let a = fr.stack.pop().unwrap_or(Value::Null);
                    let off = fr.i16();
                    let same = match (&a, &b) {
                        (Value::Null, Value::Null) => true,
                        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y),
                        (Value::Obj(x), Value::Obj(y)) => Rc::ptr_eq(x, y),
                        (Value::Array(x), Value::Array(y)) => Rc::ptr_eq(x, y),
                        _ => false,
                    };
                    let taken = if op == 0xA5 { same } else { !same };
                    if taken {
                        fr.pc = (fr.pc as i32 + off as i32 - 3) as usize;
                    }
                }
                0xA7 => {
                    let off = fr.i16();
                    fr.pc = (fr.pc as i32 + off as i32 - 3) as usize;
                }
                0xC8 => {
                    let off = fr.i32();
                    fr.pc = (fr.pc as i64 + off as i64 - 5) as usize;
                }
                0xC6 | 0xC7 => {
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    let off = fr.i16();
                    let is_null = matches!(v, Value::Null);
                    let taken = if op == 0xC6 { is_null } else { !is_null };
                    if taken {
                        fr.pc = (fr.pc as i32 + off as i32 - 3) as usize;
                    }
                }
                0xAA | 0xAB => {
                    // switch：pc 已在指令后一格；对齐到 4
                    let insn = fr.pc - 1;
                    let pad = (4 - ((insn + 1) % 4)) % 4;
                    let at = insn + 1 + pad;
                    let default = i32_at(&fr.code, at);
                    match op {
                        0xAA => {
                            let lo = i32_at(&fr.code, at + 4);
                            let hi = i32_at(&fr.code, at + 8);
                            let v = fr.stack.pop().map(|x| x.as_i() as i32).unwrap_or(0);
                            let target = if v >= lo && v <= hi {
                                i32_at(&fr.code, at + 12 + ((v - lo) as usize) * 4)
                            } else {
                                default
                            };
                            fr.pc = (insn as i32 + target) as usize;
                        }
                        _ => {
                            let n = i32_at(&fr.code, at + 4);
                            let v = fr.stack.pop().map(|x| x.as_i() as i32).unwrap_or(0);
                            let mut target = default;
                            for k in 0..n {
                                if i32_at(&fr.code, at + 8 + k as usize * 8) == v {
                                    target = i32_at(&fr.code, at + 8 + k as usize * 8 + 4);
                                    break;
                                }
                            }
                            fr.pc = (insn as i32 + target) as usize;
                        }
                    }
                }
                /* ── 数组元素加载/存储 ── */
                0x2E..=0x35 => {
                    // xaload：弹 index、arrayref，压元素（容器模型返回默认值）
                    fr.stack.pop();
                    fr.stack.pop();
                    fr.stack.push(Value::Int(0));
                }
                0x4F..=0x56 => {
                    // xastore：弹 value、index、arrayref（容器模型不实际存储）
                    fr.stack.pop();
                    fr.stack.pop();
                    fr.stack.pop();
                }
                /* ── 返回 ── */
                0xAC | 0xAD | 0xAE | 0xAF | 0xB0 => return Ok(fr.stack.pop()),
                0xB1 => return Ok(None),
                /* ── 引用与字段 ── */
                0xB2 => {
                    let idx = fr.u16();
                    let target = fr.cf.describe_member(idx);
                    if target.starts_with("java/lang/System.out:") {
                        self.stats.shim_calls += 1;
                        fr.stack.push(self.new_obj("java/io/PrintStream"));
                        continue;
                    }
                    let (class, rest) = target.split_once('.').unwrap_or(("", ""));
                    let (fname, fdesc) = rest.split_once(':').unwrap_or(("", ""));
                    self.ensure_init(class, fr)?;
                    let key = (class.to_string(), format!("{fname}{fdesc}"));
                    match self.statics.get(&key) {
                        Some(v) => fr.stack.push(v.clone()),
                        None => {
                            if self.store.borrow().has(class) {
                                // jar 内类但字段未赋值：putstatic 尚未执行——给默认值并继续
                                fr.stack.push(default_value(fdesc));
                            } else {
                                return Err(self.wall("MissingClass", format!("getstatic {target}——类不在 client jar（JDK/libraries）"), &[fr.clone()]));
                            }
                        }
                    }
                }
                0xB3 => {
                    let idx = fr.u16();
                    let target = fr.cf.describe_member(idx);
                    let (class, rest) = target.split_once('.').unwrap_or(("", ""));
                    let (fname, fdesc) = rest.split_once(':').unwrap_or(("", ""));
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    let key = (class.to_string(), format!("{fname}{fdesc}"));
                    self.statics.insert(key, v);
                }
                0xB4 => {
                    let idx = fr.u16();
                    let target = fr.cf.describe_member(idx);
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    fr.stack.push(field_of(&v, &target));
                }
                0xB5 => {
                    let idx = fr.u16();
                    let target = fr.cf.describe_member(idx);
                    let v = fr.stack.pop().unwrap_or(Value::Null);
                    let o = fr.stack.pop().unwrap_or(Value::Null);
                    put_field(o, &target, v);
                }
                /* ── 调用 ── */
                0xB6 | 0xB7 | 0xB8 | 0xB9 => {
                    let idx = fr.u16();
                    if op == 0xB9 {
                        fr.u8();
                        fr.u8(); // count + 0
                    }
                    let target = fr.cf.describe_member(idx);
                    let (tclass, rest) = target.split_once('.').unwrap_or(("", ""));
                    let (tname, tdesc) = rest.split_once(':').unwrap_or(("", ""));
                    let widths = param_widths(tdesc);
                    let mut args: Vec<Value> = Vec::with_capacity(widths.len() + 1);
                    // 右往左弹：本模型里每个参数占 1 个栈 Value（wide 参数的
                    // 「两槽」只体现在 locals 布局，操作数栈上是单个值）。
                    for _ in 0..widths.len() {
                        args.push(fr.stack.pop().unwrap_or(Value::Null));
                    }
                    args.reverse();
                    let has_ret = !returns_void(tdesc);
                    if op == 0xB8 {
                        self.ensure_init(tclass, fr)?;
                        let ret = self.call_static(tclass, tname, tdesc, args)?;
                        if has_ret {
                            fr.stack.push(ret.unwrap_or(Value::Null));
                        }
                    } else {
                        let receiver = args.first().cloned().unwrap_or(Value::Null);
                        let ret = self.call_virtual(&receiver, tclass, tname, tdesc, args)?;
                        if has_ret {
                            fr.stack.push(ret.unwrap_or(Value::Null));
                        }
                    }
                }
                0xBA => {
                    let idx = fr.u16();
                    let target = fr.cf.describe_member(idx);
                    let bsm = fr
                        .cf
                        .bootstrap_methods
                        .first()
                        .map(|(d, _)| d.clone())
                        .unwrap_or_default();
                    return Err(self.wall(
                        "InvokeDynamic",
                        format!("invokedynamic {target}——引导方法：{bsm}（LambdaMetafactory / StringConcatFactory，需要完整 java.lang.invoke 运行时）"),
                        &[fr.clone()],
                    ));
                }
                0xBB => {
                    let idx = fr.u16();
                    let cname = fr.cf.cp_class(idx).unwrap_or_default();
                    self.ensure_init(&cname, fr)?;
                    fr.stack.push(self.new_obj(&cname));
                }
                0xBC => {
                    let _at = fr.u8();
                    fr.stack.push(Value::Array(Rc::new(RefCell::new(Vec::new()))));
                }
                0xBD => {
                    let idx = fr.u16();
                    let _ = fr.cf.cp_class(idx);
                    fr.stack.push(Value::Array(Rc::new(RefCell::new(Vec::new()))));
                }
                0xBE => {
                    let a = fr.stack.pop().unwrap_or(Value::Null);
                    fr.stack.push(Value::Int(a.as_i() as i32));
                }
                0xBF => {
                    return Err(self.wall("Throw", "athrow——异常传播未实现（真实 JVM 需完整异常表 + finally 语义）".to_string(), &[fr.clone()]));
                }
                0xC0 => {
                    let idx = fr.u16();
                    let _ = fr.cf.cp_class(idx); // checkcast 原型放行（无类型系统）
                }
                0xC1 => {
                    let idx = fr.u16();
                    let _ = fr.cf.cp_class(idx);
                    fr.stack.pop();
                    fr.stack.push(Value::Int(0)); // instanceof 一律 false（无类型系统）
                }
                0xC2 | 0xC3 => {} // monitor：单线程原型直过
                /* ── 其余：wide / multianewarray / jsr/ret 等 ── */
                _ => {
                    return Err(self.wall(
                        "UnsupportedOpcode",
                        format!("opcode {op:#04x}（{}）未实现——解释器 opcode 覆盖率是实验度量", op_name(op)),
                        &[fr.clone()],
                    ));
                }
            }
        }
    }

    fn ldc(&mut self, fr: &mut Frame, idx: u16) -> Result<(), Stop> {
        let cf = &fr.cf;
        match cf.cp.get(idx as usize) {
            Some(Some(CpInfo::Integer(v))) => fr.stack.push(Value::Int(*v)),
            Some(Some(CpInfo::Long(v))) => fr.stack.push(Value::Long(*v)),
            Some(Some(CpInfo::Float(v))) => fr.stack.push(Value::Float(*v)),
            Some(Some(CpInfo::Double(v))) => fr.stack.push(Value::Double(*v)),
            Some(Some(CpInfo::String { string_index })) => {
                let s = cf.cp_utf8(*string_index).unwrap_or_default();
                fr.stack.push(Value::Str(Rc::new(s)));
            }
            Some(Some(CpInfo::Class { name_index })) => {
                let s = cf.cp_utf8(*name_index).unwrap_or_default();
                fr.stack.push(Value::ClassRef(s));
            }
            Some(Some(CpInfo::MethodType { descriptor_index })) => {
                let d = cf.cp_utf8(*descriptor_index).unwrap_or_default();
                return Err(self.wall("InvokeDynamic", format!("ldc MethodType {d}（java.lang.invoke 体系）"), &[fr.clone()]));
            }
            Some(Some(CpInfo::MethodHandle { kind, ref_index })) => {
                return Err(self.wall(
                    "InvokeDynamic",
                    format!("ldc MethodHandle kind={kind} → {}（java.lang.invoke 体系）", cf.describe_member(*ref_index)),
                    &[fr.clone()],
                ));
            }
            _ => fr.stack.push(Value::Null),
        }
        Ok(())
    }

    fn new_obj(&self, class: &str) -> Value {
        Value::Obj(Rc::new(RefCell::new(HeapObj { class: class.to_string(), fields: HashMap::new() })))
    }

    fn pop2_i(&mut self, fr: &mut Frame) -> (i64, i64) {
        let b = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
        let a = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
        (a, b)
    }
    fn pop2_ii(&mut self, fr: &mut Frame) -> (i32, i32) {
        let b = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0) as i32;
        let a = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0) as i32;
        (a, b)
    }
    fn pop2_l(&mut self, fr: &mut Frame) -> (i64, i64) {
        self.pop2_i(fr)
    }

    fn arith(&mut self, fr: &mut Frame, op: u8) {
        match op {
            0x60..=0x64 => {
                let b = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0) as i32;
                let a = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0) as i32;
                let r = match op {
                    0x60 => a.wrapping_add(b),
                    0x61 => a.wrapping_sub(b),
                    0x62 => a.wrapping_mul(b),
                    0x63 => {
                        if b == 0 {
                            0
                        } else {
                            a.wrapping_div(b)
                        }
                    }
                    _ => {
                        if b == 0 {
                            0
                        } else {
                            a.wrapping_rem(b)
                        }
                    }
                };
                fr.stack.push(Value::Int(r));
            }
            0x65..=0x69 => {
                let b = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
                let a = fr.stack.pop().map(|x| x.as_i()).unwrap_or(0);
                let r = match op {
                    0x65 => a.wrapping_add(b),
                    0x66 => a.wrapping_sub(b),
                    0x67 => a.wrapping_mul(b),
                    0x68 => {
                        if b == 0 {
                            0
                        } else {
                            a.wrapping_div(b)
                        }
                    }
                    _ => {
                        if b == 0 {
                            0
                        } else {
                            a.wrapping_rem(b)
                        }
                    }
                };
                fr.stack.push(Value::Long(r));
            }
            _ => {
                // float/double
                let b = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0);
                let a = fr.stack.pop().map(|x| x.as_f()).unwrap_or(0.0);
                let r = match op {
                    0x6A | 0x6F => a + b,
                    0x6B | 0x70 => a - b,
                    0x6C | 0x71 => a * b,
                    0x6D | 0x72 => a / b,
                    _ => a % b,
                };
                if matches!(op, 0x6A..=0x6E) {
                    fr.stack.push(Value::Float(r as f32));
                } else {
                    fr.stack.push(Value::Double(r));
                }
            }
        }
    }

    /* ── JDK shim 面 ── */
    fn jdk_static(&mut self, class: &str, method: &str, desc: &str, args: &[Value]) -> Option<Option<Value>> {
        let _ = desc;
        match (class, method) {
            ("java/lang/System", "arraycopy") => {
                self.stats.shim_calls += 1;
                Some(Some(Value::Null))
            }
            ("java/lang/System", "currentTimeMillis") | ("java/lang/System", "nanoTime") => {
                self.stats.shim_calls += 1;
                Some(Some(Value::Long(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                )))
            }
            ("java/lang/Math", "min") | ("java/lang/Math", "max") => {
                self.stats.shim_calls += 1;
                let b = args.get(1).map(|x| x.as_i()).unwrap_or(0);
                let a = args.first().map(|x| x.as_i()).unwrap_or(0);
                let r = if method == "min" { a.min(b) } else { a.max(b) };
                Some(Some(Value::Int(r as i32)))
            }
            ("java/lang/Integer", "parseInt") => {
                self.stats.shim_calls += 1;
                let s = match args.first() {
                    Some(Value::Str(s)) => s.as_str(),
                    _ => "",
                };
                Some(Some(Value::Int(s.parse().unwrap_or(0))))
            }
            ("java/lang/Integer", "valueOf") => {
                self.stats.shim_calls += 1;
                let n = match args.first() {
                    Some(Value::Int(v)) => *v,
                    Some(Value::Str(s)) => s.parse().unwrap_or(0),
                    _ => 0,
                };
                Some(Some(Value::Int(n)))
            }
            ("java/lang/String", "valueOf") => {
                self.stats.shim_calls += 1;
                let s = match args.first() {
                    Some(v) => match v {
                        Value::Int(i) => i.to_string(),
                        Value::Long(l) => l.to_string(),
                        Value::Str(s) => s.as_str().to_string(),
                        _ => v.describe(),
                    },
                    None => String::new(),
                };
                Some(Some(Value::Str(Rc::new(s))))
            }
            _ => None,
        }
    }

    fn jdk_virtual(&mut self, receiver: &Value, owner: &str, method: &str, desc: &str, args: &[Value]) -> Option<Option<Value>> {
        let _ = desc;
        match (owner, method) {
            ("java/io/PrintStream", "println") | ("java/io/PrintStream", "print") => {
                self.stats.shim_calls += 1;
                let text = match args.first().or(if args.is_empty() { Some(receiver) } else { None }) {
                    Some(Value::Str(s)) => s.as_str().to_string(),
                    Some(v) => match v {
                        Value::Int(i) => i.to_string(),
                        Value::Long(l) => l.to_string(),
                        _ => v.describe(),
                    },
                    None => String::new(),
                };
                if self.stats.stdout_lines.len() < 50 {
                    self.stats.stdout_lines.push(text.clone());
                }
                println!("  [游戏 stdout] {text}");
                Some(Some(Value::Null))
            }
            ("java/lang/String", "equals") => {
                self.stats.shim_calls += 1;
                let a = match receiver {
                    Value::Str(s) => s.as_str().to_string(),
                    _ => String::new(),
                };
                let b = match args.first() {
                    Some(Value::Str(s)) => s.as_str().to_string(),
                    _ => String::new(),
                };
                Some(Some(Value::Int((a == b) as i32)))
            }
            ("java/lang/String", "length") => {
                self.stats.shim_calls += 1;
                let n = match receiver {
                    Value::Str(s) => s.len() as i32,
                    _ => 0,
                };
                Some(Some(Value::Int(n)))
            }
            ("java/lang/Object", "<init>") => {
                self.stats.shim_calls += 1;
                Some(Some(Value::Null))
            }
            (_, "toString") if args.is_empty() => {
                self.stats.shim_calls += 1;
                Some(Some(Value::Str(Rc::new(receiver.describe()))))
            }
            ("java/lang/StringBuilder", "append") => {
                self.stats.shim_calls += 1;
                // 简化：把 append 的内容拼进对象 hidden 字段
                let text = match args.first() {
                    Some(Value::Str(s)) => s.as_str().to_string(),
                    Some(v) => v.describe(),
                    None => String::new(),
                };
                if let Value::Obj(o) = receiver {
                    let mut b = o.borrow_mut();
                    let cur = b.fields.get("__sb").cloned().unwrap_or(Value::Str(Rc::new(String::new())));
                    if let Value::Str(s) = cur {
                        let mut n = String::from(s.as_str());
                        n.push_str(&text);
                        b.fields.insert("__sb".into(), Value::Str(Rc::new(n)));
                    }
                }
                Some(Some(receiver.clone()))
            }
            ("java/lang/StringBuilder", "toString") => {
                self.stats.shim_calls += 1;
                let s = if let Value::Obj(o) = receiver {
                    match o.borrow().fields.get("__sb") {
                        Some(Value::Str(s)) => s.as_str().to_string(),
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };
                Some(Some(Value::Str(Rc::new(s))))
            }
            _ => None,
        }
    }
}

fn i32_at(code: &[u8], at: usize) -> i32 {
    if at + 4 > code.len() {
        return 0;
    }
    i32::from_be_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]])
}

fn field_of(v: &Value, target: &str) -> Value {
    if let Value::Obj(o) = v {
        if let Some(x) = o.borrow().fields.get(target) {
            return x.clone();
        }
    }
    default_value(target.rsplit(':').next().unwrap_or(""))
}

fn put_field(target: Value, spec: &str, v: Value) {
    if let Value::Obj(o) = target {
        o.borrow_mut().fields.insert(spec.to_string(), v);
    }
}

fn default_value(desc: &str) -> Value {
    match desc.chars().next() {
        Some('J') => Value::Long(0),
        Some('F') => Value::Float(0.0),
        Some('D') => Value::Double(0.0),
        Some('I') | Some('S') | Some('B') | Some('C') | Some('Z') => Value::Int(0),
        _ => Value::Null,
    }
}

fn locals_needed(desc: &str, has_this: bool) -> usize {
    let mut n = if has_this { 1 } else { 0 };
    let open = desc.find('(').map(|i| i + 1).unwrap_or(0);
    let close = desc.find(')').unwrap_or(open);
    let mut chars = desc[open..close].chars();
    while let Some(c) = chars.next() {
        match c {
            'J' | 'D' => n += 2,
            'L' => {
                while chars.next() != Some(';') {}
                n += 1;
            }
            '[' => {
                // 数组：吞掉维度与元素类型
                while chars.next() == Some('[') {}
                if let Some('L') = chars.clone().next() {
                    while chars.next() != Some(';') {}
                }
                n += 1;
            }
            ')' => break,
            _ => n += 1,
        }
    }
    n
}

fn returns_void(desc: &str) -> bool {
    desc.rsplit(':').next().unwrap_or(desc).ends_with(")V")
}

/// 参数槽宽列表（每个元素 1 或 2，用于实参装填与出参计数）。
fn param_widths(desc: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let open = desc.find('(').map(|i| i + 1).unwrap_or(0);
    let close = desc.find(')').unwrap_or(open);
    let mut chars = desc[open..close].chars();
    while let Some(c) = chars.next() {
        match c {
            'J' | 'D' => out.push(2),
            'L' => {
                while chars.next() != Some(';') {}
                out.push(1);
            }
            '[' => {
                while chars.clone().next() == Some('[') {
                    chars.next();
                }
                if chars.clone().next() == Some('L') {
                    while chars.next() != Some(';') {}
                } else {
                    chars.next();
                }
                out.push(1);
            }
            ')' => break,
            _ => out.push(1),
        }
    }
    out
}

fn op_name(op: u8) -> &'static str {
    match op {
        0xC4 => "wide",
        0xC5 => "multianewarray",
        0xA8 => "jsr",
        0xA9 => "ret",
        _ => "?",
    }
}
