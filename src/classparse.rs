//! Java class 文件解析器（JVM 规范 JVMS §4）：
//! 魔数 / 版本 / 常量池（17 种 tag 全覆盖）/ 访问标志 / 字段 / 方法 / 属性。
//! 这是「Rust 转译实验」的地基：解析成功才有资格谈解释执行。

use std::collections::HashMap;
use std::fmt::Write as _;

#[allow(dead_code)]
pub const ACC_PUBLIC: u16 = 0x0001;
pub const ACC_STATIC: u16 = 0x0008;
pub const ACC_NATIVE: u16 = 0x0100;
pub const ACC_ABSTRACT: u16 = 0x0400;
pub const ACC_INTERFACE: u16 = 0x0200;

#[derive(Debug, Clone, PartialEq)]
pub enum CpInfo {
    Utf8(String),
    Class { name_index: u16 },
    String { string_index: u16 },
    NameAndType { name_index: u16, descriptor_index: u16 },
    Fieldref { class_index: u16, nat_index: u16 },
    Methodref { class_index: u16, nat_index: u16 },
    InterfaceMethodref { class_index: u16, nat_index: u16 },
    MethodHandle { kind: u8, ref_index: u16 },
    MethodType { descriptor_index: u16 },
    Dynamic { bsm_index: u16, nat_index: u16 },
    InvokeDynamic { bsm_index: u16, nat_index: u16 },
    Integer(i32),
    Long(i64),
    Float(f32),
    Double(f64),
}

#[derive(Debug, Clone)]
pub struct Field {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
}

#[derive(Debug, Clone)]
pub struct Method {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
    /// Code 属性存在时的字节码；None = abstract/native（无 code）。
    pub code: Option<Vec<u8>>,
    pub max_stack: u16,
    pub max_locals: u16,
    /// 异常表条目：(start, end, handler, catch_type 类名)。
    pub exceptions: Vec<(u16, u16, u16, Option<String>)>,
    /// bootstrap 方法引用（BootstrapMethods 属性 → 方法句柄描述）。
    pub visible_annotations_desc: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClassFile {
    pub minor: u16,
    pub major: u16,
    pub access: u16,
    /// this/super 类名（内部名，如 net/minecraft/client/main/Main）。
    pub this_class: String,
    pub super_class: Option<String>,
    pub interfaces: Vec<String>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub cp: Vec<Option<CpInfo>>,
    /// BootstrapMethods 属性：(方法句柄描述, 参数常量池索引列表)。
    pub bootstrap_methods: Vec<(String, Vec<u16>)>,
}

#[derive(Debug)]
pub struct ParseError {
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

pub fn parse(bytes: &[u8]) -> Result<ClassFile, ParseError> {
    let mut r = Reader { b: bytes, p: 0 };
    let magic = r.u4()?;
    if magic != 0xCAFEBABE {
        return Err(err(&format!("魔数不是 CAFEBABE（{magic:#010x}）")));
    }
    let minor = r.u2()?;
    let major = r.u2()?;
    let cp_count = r.u2()? as usize;
    let mut cp: Vec<Option<CpInfo>> = vec![None; cp_count];
    let mut i = 1usize;
    while i < cp_count {
        let tag = r.u1()?;
        match tag {
            1 => {
                let len = r.u2()? as usize;
                let raw = r.bytes(len)?;
                cp[i] = Some(CpInfo::Utf8(decode_modified_utf8(raw)));
            }
            3 => cp[i] = Some(CpInfo::Integer(r.u4()? as i32)),
            4 => cp[i] = Some(CpInfo::Float(f32::from_bits(r.u4()?))),
            5 => {
                let hi = r.u4()? as u64;
                let lo = r.u4()? as u64;
                cp[i] = Some(CpInfo::Long(((hi << 32) | lo) as i64));
                i += 1; // long/double 占两槽
            }
            6 => {
                let hi = r.u4()? as u64;
                let lo = r.u4()? as u64;
                cp[i] = Some(CpInfo::Double(f64::from_bits((hi << 32) | lo)));
                i += 1;
            }
            7 => cp[i] = Some(CpInfo::Class { name_index: r.u2()? }),
            8 => cp[i] = Some(CpInfo::String { string_index: r.u2()? }),
            9 => {
                let (a, b) = r.u2x2()?;
                cp[i] = Some(CpInfo::Fieldref { class_index: a, nat_index: b });
            }
            10 => {
                let (a, b) = r.u2x2()?;
                cp[i] = Some(CpInfo::Methodref { class_index: a, nat_index: b });
            }
            11 => {
                let (a, b) = r.u2x2()?;
                cp[i] = Some(CpInfo::InterfaceMethodref { class_index: a, nat_index: b });
            }
            12 => {
                let (a, b) = r.u2x2()?;
                cp[i] = Some(CpInfo::NameAndType { name_index: a, descriptor_index: b });
            }
            15 => {
                let kind = r.u1()?;
                let idx = r.u2()?;
                cp[i] = Some(CpInfo::MethodHandle { kind, ref_index: idx });
            }
            16 => cp[i] = Some(CpInfo::MethodType { descriptor_index: r.u2()? }),
            17 => {
                let (a, b) = r.u2x2()?;
                cp[i] = Some(CpInfo::Dynamic { bsm_index: a, nat_index: b });
            }
            18 => {
                let (a, b) = r.u2x2()?;
                cp[i] = Some(CpInfo::InvokeDynamic { bsm_index: a, nat_index: b });
            }
            19 => cp[i] = Some(CpInfo::Class { name_index: r.u2()? }),  // Module
            20 => cp[i] = Some(CpInfo::Class { name_index: r.u2()? }),  // Package
            t => return Err(err(&format!("未知常量池 tag {t}（index {i}）"))),
        }
        i += 1;
    }

    let access = r.u2()?;
    let this_idx = r.u2()?;
    let super_idx = r.u2()?;
    let this_class = cp_class_name(&cp, this_idx).unwrap_or_default();
    let super_class = if super_idx == 0 { None } else { cp_class_name(&cp, super_idx) };
    let icount = r.u2()? as usize;
    let mut interfaces = Vec::with_capacity(icount);
    for _ in 0..icount {
        if let Some(n) = cp_class_name(&cp, r.u2()?) {
            interfaces.push(n);
        }
    }

    let fields = parse_members(&mut r, &cp)?;
    let methods = parse_methods(&mut r, &cp)?;

    // 类级属性（BootstrapMethods 关键；其余跳过）。
    let acount = r.u2()? as usize;
    let mut bootstrap_methods = Vec::new();
    for _ in 0..acount {
        let name_idx = r.u2()?;
        let len = r.u4()? as usize;
        let attr_name = cp_utf8(&cp, name_idx).unwrap_or_default();
        let body = r.bytes(len)?;
        if attr_name == "BootstrapMethods" {
            let mut br = Reader { b: &body, p: 0 };
            let n = br.u2()? as usize;
            for _ in 0..n {
                let bsm_ref = br.u2()?;
                let argc = br.u2()? as usize;
                let mut args = Vec::with_capacity(argc);
                for _ in 0..argc {
                    args.push(br.u2()?);
                }
                let handle_desc = cp
                    .get(bsm_ref as usize)
                    .and_then(|x| x.as_ref())
                    .map(|c| match c {
                        CpInfo::MethodHandle { kind, ref_index } => {
                            format!("handle kind={kind} -> {}", describe_member(&cp, *ref_index))
                        }
                        other => format!("{other:?}"),
                    })
                    .unwrap_or_default();
                bootstrap_methods.push((handle_desc, args));
            }
        }
    }

    Ok(ClassFile { minor, major, access, this_class, super_class, interfaces, fields, methods, cp, bootstrap_methods })
}

fn err(msg: &str) -> ParseError {
    ParseError { msg: msg.to_string() }
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn u1(&mut self) -> Result<u8, ParseError> {
        if self.p >= self.b.len() {
            return Err(err("越界：读 u1"));
        }
        let v = self.b[self.p];
        self.p += 1;
        Ok(v)
    }
    fn u2(&mut self) -> Result<u16, ParseError> {
        let a = self.u1()? as u16;
        let b = self.u1()? as u16;
        Ok((a << 8) | b)
    }
    fn u4(&mut self) -> Result<u32, ParseError> {
        let a = self.u2()? as u32;
        let b = self.u2()? as u32;
        Ok((a << 16) | b)
    }
    fn u2x2(&mut self) -> Result<(u16, u16), ParseError> {
        Ok((self.u2()?, self.u2()?))
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        if self.p + n > self.b.len() {
            return Err(err(&format!("越界：读 {n} 字节（剩 {}）", self.b.len() - self.p)));
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
}

/// JVM 修正版 UTF-8（0xC0 0x80 编码 NUL，代理对直接编码）。
fn decode_modified_utf8(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    while i < raw.len() {
        let b = raw[i];
        match b {
            0x01..=0x7F => {
                out.push(b as char);
                i += 1;
            }
            0xC0..=0xDF if i + 1 < raw.len() => {
                let cp = ((b as u32 & 0x1F) << 6) | (raw[i + 1] as u32 & 0x3F);
                if let Some(c) = char::from_u32(cp) {
                    out.push(c);
                }
                i += 2;
            }
            0xE0..=0xEF if i + 2 < raw.len() => {
                let cp = ((b as u32 & 0x0F) << 12)
                    | ((raw[i + 1] as u32 & 0x3F) << 6)
                    | (raw[i + 2] as u32 & 0x3F);
                if let Some(c) = char::from_u32(cp) {
                    out.push(c);
                }
                i += 3;
            }
            _ => {
                // 非法序列：原样丢弃（解析器尽力而为）
                i += 1;
            }
        }
    }
    out
}

fn cp_utf8(cp: &[Option<CpInfo>], idx: u16) -> Option<String> {
    match cp.get(idx as usize)? {
        Some(CpInfo::Utf8(s)) => Some(s.clone()),
        _ => None,
    }
}

fn cp_class_name(cp: &[Option<CpInfo>], idx: u16) -> Option<String> {
    match cp.get(idx as usize)? {
        Some(CpInfo::Class { name_index }) => cp_utf8(cp, *name_index),
        _ => None,
    }
}

/// Methodref/Fieldref → "类.名:描述" 描述串。
fn describe_member(cp: &[Option<CpInfo>], ref_index: u16) -> String {
    let (class, nat) = match cp.get(ref_index as usize) {
        Some(Some(CpInfo::Methodref { class_index, nat_index }))
        | Some(Some(CpInfo::InterfaceMethodref { class_index, nat_index }))
        | Some(Some(CpInfo::Fieldref { class_index, nat_index })) => (*class_index, *nat_index),
        _ => return "?".into(),
    };
    let cname = cp_class_name(cp, class).unwrap_or_default();
    let (n, d) = match cp.get(nat as usize) {
        Some(Some(CpInfo::NameAndType { name_index, descriptor_index })) => (
            cp_utf8(cp, *name_index).unwrap_or_default(),
            cp_utf8(cp, *descriptor_index).unwrap_or_default(),
        ),
        _ => (String::new(), String::new()),
    };
    format!("{cname}.{n}:{d}")
}

fn parse_members(r: &mut Reader, cp: &[Option<CpInfo>]) -> Result<Vec<Field>, ParseError> {
    let count = r.u2()? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let access = r.u2()?;
        let name = cp_utf8(cp, r.u2()?).unwrap_or_default();
        let descriptor = cp_utf8(cp, r.u2()?).unwrap_or_default();
        let acount = r.u2()? as usize;
        for _ in 0..acount {
            let _an = r.u2()?;
            let alen = r.u4()? as usize;
            r.bytes(alen)?;
        }
        out.push(Field { access, name, descriptor });
    }
    Ok(out)
}

fn parse_methods(r: &mut Reader, cp: &[Option<CpInfo>]) -> Result<Vec<Method>, ParseError> {
    let count = r.u2()? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let access = r.u2()?;
        let name = cp_utf8(cp, r.u2()?).unwrap_or_default();
        let descriptor = cp_utf8(cp, r.u2()?).unwrap_or_default();
        let acount = r.u2()? as usize;
        let mut code = None;
        let mut max_stack = 0u16;
        let mut max_locals = 0u16;
        let mut exceptions = Vec::new();
        let mut annos = Vec::new();
        for _ in 0..acount {
            let an = r.u2()?;
            let alen = r.u4()? as usize;
            let an_name = cp_utf8(cp, an).unwrap_or_default();
            let body = r.bytes(alen)?;
            match an_name.as_str() {
                "Code" => {
                    let mut br = Reader { b: body, p: 0 };
                    max_stack = br.u2()?;
                    max_locals = br.u2()?;
                    let clen = br.u4()? as usize;
                    code = Some(br.bytes(clen)?.to_vec());
                    let ec = br.u2()? as usize;
                    for _ in 0..ec {
                        let (s, e, h) = (br.u2()?, br.u2()?, br.u2()?);
                        let ct = br.u2()?;
                        let ct_name = if ct == 0 { None } else { cp_class_name(cp, ct) };
                        exceptions.push((s, e, h, ct_name));
                    }
                }
                "RuntimeVisibleAnnotations" | "RuntimeInvisibleAnnotations" => {
                    // 粗略提取注解类型描述符（type_index 在开头 u2）。
                    if body.len() >= 2 {
                        let tidx = ((body[0] as u16) << 8) | body[1] as u16;
                        if let Some(d) = cp_utf8(cp, tidx) {
                            annos.push(d);
                        }
                    }
                }
                _ => {}
            }
        }
        out.push(Method {
            access,
            name,
            descriptor,
            code,
            max_stack,
            max_locals,
            exceptions,
            visible_annotations_desc: annos,
        });
    }
    Ok(out)
}

impl ClassFile {
    /// 常量池字符串引用（ldc 用）。
    pub fn cp_string(&self, idx: u16) -> Option<String> {
        match self.cp.get(idx as usize)? {
            Some(CpInfo::String { string_index }) => self.cp_utf8(*string_index),
            Some(CpInfo::Utf8(s)) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn cp_utf8(&self, idx: u16) -> Option<String> {
        cp_utf8(&self.cp, idx)
    }

    /// 方法/字段引用描述："类.名:描述"。
    pub fn describe_member(&self, idx: u16) -> String {
        describe_member(&self.cp, idx)
    }

    /// 类引用名。
    pub fn cp_class(&self, idx: u16) -> Option<String> {
        cp_class_name(&self.cp, idx)
    }

    pub fn find_method(&self, name: &str, descriptor: &str) -> Option<&Method> {
        self.methods
            .iter()
            .find(|m| m.name == name && m.descriptor == descriptor)
    }

    /// jar 内部名转点分名（打印用）。
    pub fn dotted_name(&self) -> String {
        self.this_class.replace('/', ".")
    }

    /// 全部方法的字节码指令计数（静态统计用）。
    pub fn total_instructions(&self) -> usize {
        self.methods.iter().filter_map(|m| m.code.as_ref()).map(|c| c.len()).sum()
    }

    /// 静态特性扫描：生成该类的特征统计。
    pub fn scan_features(&self) -> FeatureStats {
        let mut st = FeatureStats { classes: 1, ..Default::default() };
        st.major_version = self.major;
        if self.major > st.max_class_major {
            st.max_class_major = self.major;
        }
        for m in &self.methods {
            if m.access & ACC_NATIVE != 0 {
                st.native_methods += 1;
                let cname = self.this_class.clone();
                let sample = st
                    .native_samples
                    .entry(cname)
                    .or_default();
                if sample.len() < 3 {
                    sample.push(format!("{}{}", m.name, m.descriptor));
                }
            }
            let Some(code) = &m.code else { continue };
            let mut i = 0usize;
            while i < code.len() {
                let op = code[i];
                match op {
                    // invokedynamic (0xBA)：5 字节
                    0xBA => {
                        st.invokedynamic_sites += 1;
                        if i + 3 < code.len() {
                            let idx = ((code[i + 1] as u16) << 8) | code[i + 2] as u16;
                            if let Some(&Some(CpInfo::InvokeDynamic { bsm_index, nat_index })) =
                                self.cp.get(idx as usize)
                            {
                                if st.bsm_samples.len() < 8 {
                                    st.bsm_samples.push(format!(
                                        "bsm={} nat={}",
                                        self.bootstrap_methods
                                            .get(bsm_index as usize)
                                            .map(|(d, _)| d.clone())
                                            .unwrap_or_default(),
                                        self.describe_member(nat_index)
                                    ));
                                }
                            }
                        }
                        i += 5;
                    }
                    // tableswitch / lookupswitch：变长（4 字节对齐 + 偏移表）
                    0xAA | 0xAB => {
                        let pad = (4 - ((i + 1) % 4)) % 4;
                        if op == 0xAA {
                            if i + 1 + pad + 12 <= code.len() {
                                let lo = u32_at(code, i + 1 + pad + 4);
                                let hi = u32_at(code, i + 1 + pad + 8);
                                let n = hi.wrapping_sub(lo).saturating_add(1) as usize;
                                i += 1 + pad + 12 + n * 4;
                            } else {
                                break;
                            }
                        } else {
                            if i + 1 + pad + 8 <= code.len() {
                                let npairs = u32_at(code, i + 1 + pad + 4) as usize;
                                i += 1 + pad + 8 + npairs * 8;
                            } else {
                                break;
                            }
                        }
                    }
                    // invoke 系列 + 带常量池索引的 3 字节指令
                    0xB2..=0xB9 | 0xBB | 0xBD | 0x13 | 0x14 => i += 3,
                    0x12 => i += 2,          // ldc
                    0x10 => i += 2,          // bipush
                    0x11 => i += 3,          // sipush
                    0x99..=0xA8 | 0xC0 | 0xC1 | 0xC6 | 0xC7 => i += 3, // 分支/checkcast 等
                    0x15..=0x19 | 0x2F..=0x4E | 0x1A..=0x2E => i += 1, // xload_n/xstore_n/常量入栈
                    0x54..=0x77 => i += 1,   // xstore_n
                    0x84 => i += 3,          // iinc
                    0xC8 | 0xC9 => i += 5,   // goto_w / jsr_w
                    0xC4 => break,           // wide：保守终止本方法扫描（不影响统计正确性主线）
                    _ => i += 1,             // 其余单字节
                }
            }
        }
        st
    }
}

fn u32_at(code: &[u8], at: usize) -> u32 {
    ((code[at] as u32) << 24) | ((code[at + 1] as u32) << 16) | ((code[at + 2] as u32) << 8) | code[at + 3] as u32
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct FeatureStats {
    pub classes: usize,
    pub major_version: u16,
    pub max_class_major: u16,
    pub native_methods: usize,
    pub invokedynamic_sites: usize,
    /// native 方法样例（每类最多 3 条，防报告爆炸）。
    pub native_samples: HashMap<String, Vec<String>>,
    pub bsm_samples: Vec<String>,
}

impl FeatureStats {
    pub fn merge(&mut self, other: &FeatureStats) {
        self.classes += other.classes;
        self.major_version = other.major_version;
        if other.max_class_major > self.max_class_major {
            self.max_class_major = other.max_class_major;
        }
        self.native_methods += other.native_methods;
        self.invokedynamic_sites += other.invokedynamic_sites;
    }
}

/// 供 scan/interp 打印：把类摘要写成文本行。
pub fn summarize(c: &ClassFile) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "class {} (major {}) methods={} fields={}",
        c.dotted_name(), c.major, c.methods.len(), c.fields.len());
    s
}
