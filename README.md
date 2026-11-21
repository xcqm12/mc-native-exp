# mc-native-exp · MC 原生启动实验

> 实验问题：**从官方源下载全套 Minecraft 文件后，能否用 Rust 解释/转译字节码、启动时不再依赖 JVM？**
> 前提：必须使用微软正版账号（实验会走完整 MSA 设备码认证 + Java 版所有权校验，无正版拒绝继续）。

## 用法

```powershell
# 1) 拉官方版本清单（全部版本，公开端点）
mc-native-exp manifest

# 2) 微软正版登录（设备码流程；client id 用你已过白名单的注册，env MC_EXP_CLIENT_ID 可覆盖）
mc-native-exp auth

# 3) 下载某个版本的全套官方文件（client + libraries + assets + natives，纯官方源 + SHA1 校验）
mc-native-exp fetch 1.21.4
#    或只拿 client jar（扫描/解释用）
mc-native-exp fetch 1.21.4 --client-only

# 3b) 跨版本分析：下载全部版本的 client jar（数 GB）
mc-native-exp fetch-all-clients

# 4) 静态分析（native 方法 / invokedynamic / 依赖闭包……）
mc-native-exp scan 1.21.4
mc-native-exp scan --all

# 5) 字节码解释器原型：真刀真枪在 Rust 里解释官方 jar，记录第一堵墙
mc-native-exp interp 1.21.4

# 6) 汇总数据生成 REPORT.md
mc-native-exp report
```

## 结构

- `src/manifest.rs` — piston-meta 全版本清单
- `src/auth.rs` — MSA 设备码 → XBL → XSTS → MC token → entitlements（正版前提）
- `src/fetch.rs` — 官方源全量下载（SHA1 流式校验 + 断点续传）
- `src/classparse.rs` — 手写 Java class 文件解析器（JVMS §4，常量池 17 种 tag）
- `src/scan.rs` — 静态特性分析（跨版本）
- `src/interp.rs` — 字节码解释器原型（栈帧 + opcode 子集 + 墙检测）
- `src/report.rs` — 汇总 REPORT.md

## 安全边界

- 下载物视为不可信数据：只做 zip/字节级解析，**从不执行**；`data/` 与代码分离
- token 只落本地 `data/token.json`；不打印、不外传
- 不绕过、不伪造认证

## 结论

见 `REPORT.md`（运行 `report` 子命令自动生成；TL;DR：下载与认证完全可行，字节码解析可行，
解释执行在几百~几千条指令内撞墙——墙 = JDK 语义 + JNI + invokedynamic + 反射，
工程量以人年计；已有可行替代：Horizon（Rust 从零重写的 MC，`feature/horizon` 分支已集成）。
