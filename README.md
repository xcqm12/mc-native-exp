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

## 小七 AI 伙伴（游戏内实体）

启动器会在 **Fabric / Quilt 1.21.4** 实例启动前自动安装两件东西（内容相同不覆盖）：

- `mods/xiaoqi-companion.jar` — 客户端模组「小七」（嵌入启动器 exe，无需手动下载）
- `mods/fabric-api-0.119.4+1.21.4.jar` — 模组依赖（实例里已有任意 fabric-api 时跳过）

进世界后自动生成客户端幽灵伙伴：专属皮肤、头顶「小七」名牌、跟在玩家身后。她是**纯客户端实体**
（服务端不可见、不参与存档/战斗/同步），所以在任意服务器（包括原版服）都能用，不需要服务端装模组。

聊天框指令（本地即时生效，断网也能用）：

```
#ai 跟随      #ai 停下      #ai 过来      #ai 看我
```

其他内容（`#ai 你好呀`、`#ai 怎么做工作台`）会转发到启动器的本地 AI 桥
（`127.0.0.1` 随机端口 + 随机 Bearer token，token 只注入本机游戏进程）：

- 没配 Key → 规则兜底，提示可用指令（不报错）
- 配了 Key → OpenAI 兼容 `/chat/completions`（DeepSeek / OpenAI / 其他兼容服务）

配置方式（GUI：设置 → 小七伙伴；或命令行）：

```powershell
mc-native-exp config --ai-base-url https://api.deepseek.com/v1 --ai-key sk-... --ai-model deepseek-chat
mc-native-exp config --ai false      # 关闭小七（不装模组、不起 AI 桥）
```

限制：模组编译目标为 MC 1.21.4，其他版本实例不安装（不破坏现有模组环境）；AI 只聊天/给建议，
不会代替玩家操作游戏。

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


## 实测结果（2026-11-22 · v0.0.2.beta）

| 项 | 结果 |
|---|---|
| manifest 全版本 | ✅ 925 个（103 正式版），纯官方 piston-meta |
| 微软正版认证 | ✅ 设备码 → XBL → XSTS → entitlements 全链（SevenZeroMeow 实测通过） |
| Fabric 1.21.4 自动安装 | ✅ 安装器路线，3564 文件/322MB |
| **Fabric 启动 smoke** | ✅ **进入游戏引擎初始化**（1688 行日志，LWJGL/OpenAL/glfw 全就位） |
| Forge 1.21.4 自动安装+启动 | ✅ 安装器路线 + GA 仓库覆盖（forge 自产库在自家 maven）+ :client/:server 内嵌数据跳过 + Bootstrap 系不放原版 jar → smoke 进入引擎初始化 |
| NeoForge 安装 | 代码完成（多源回退）；实测时段 maven.neoforged.net 全源不可达，待网络窗口 |
| class 解析（26.3） | ✅ 11383/11383 全部解析（major 69 = Java 25） |
| 字节码解释器 | ✅ 首堵墙 = StringConcatFactory（invokedynamic ×32134 / 依赖闭包 jar 内 10662 类 + JDK 482 + 第三方 17134 处）——脱 JVM 差距的量化数据 |

### 本轮修复的关键 bug

- XSTS RelyingParty 必须是 `rp://api.minecraftservices.com/`（400 空体根因）
- `--data` 相对路径基于 CWD（此前落 exe 目录）
- classpath 按 maven GA 去重（loader 优先）+ natives-\* 分类器不进 classpath（jtracy/ASM 双版本冲突根因）
- 新版 natives（普通 artifact 形态）解压支持（lwjgl.dll 根因）
- 设备码 expired_token/invalid_grant 自动换码；XSTS/MC 响应重试
