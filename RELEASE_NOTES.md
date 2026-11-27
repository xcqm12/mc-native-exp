# mc-native-exp v0.1.2.beta（实验 · 可能不稳定）

> 版本方案：patch 逐次 +1 至 .9 后进位——0.1.1 → 0.1.2。

## 本版修复：Forge 安装/启动全链 + 弱链路下载

- **BMCLAPI 熔断**：镜像整体不可达时（2026-11-27 实测 bmclapi2 全站 000），
  连续失败 3 次即本次运行熔断，直接走官方/中转源——不再 63 个库 ×3 次重试刷屏
- **Forge maven 中转**：新增分发站 `/forge-relay/`（+ `/forge-cache/` 落盘缓存），
  安装器与自产库改走中转，绕开本网络到 maven.minecraftforge.net 的传输中断
- **JVM 参数全透传**：`arguments.jvm` 字符串参数不再按 `-D/-X` 前缀过滤——
  `--add-modules` / `--add-opens` 这类"选项+值"成对参数曾被拆散，报
  `--add-modules requires modules to be specified`
- **补 `${library_directory}` / `${classpath_separator}` 占位符**：缺少时 forge 的
  `-p` 模块路径全成字面量 → securejarhandler 模块找不到 → `--add-opens` 失效 →
  `InaccessibleObjectException: IMPL_LOOKUP`（本版实测修复）
- **smoke 判定加固**：只认本次启动后更新的 `latest.log`（旧冒烟日志不再造成假阳性），
  并识别 `Error: --` / `Unrecognized VM option` 等 JVM 启动失败直接判红
- 安装器版本显示修正：forge/neoforge 显示「同 loader 版本」而非 `None`；
  重试计数 `1/2` 改为真实的 `1/3`

## 实测（2026-11-27）

| 项 | 结果 |
|---|---|
| Forge 1.20.1 安装 | `install forge 1.20.1` → `1.20.1-forge-47.4.29`，EXIT=0 |
| Forge 1.20.1 启动 | `MinecraftForge v47.4.29 Initialized` / ModLauncher 10.0.9 / 资源重载完成 |
| Fabric 1.21.4 回归 | smoke 通过（Setting user + LWJGL Backend），小七伙伴链路不受影响 |
| 中转可用性 | `/forge-relay/` 回源 1 秒内 206，安装器 45+18 库一次通过 |

## 上一版：v0.1.1.beta

小七 AI 伙伴进入游戏（自动换皮 + 跟随 + `#ai` 指令 + 本地 AI 桥）与设备码验证自动复制。

## 历史：v0.1.1.beta

> 版本方案：patch 逐次 +1 至 .9 后进位——0.1.0 → 0.1.1。

## 本版新增：AI 伙伴「小七」进入游戏

启动 Fabric/Quilt **1.21.4** 实例时，启动器自动把模组装进 `mods/` 并在本机起一座 AI 桥：

- **自动装模组**：`xiaoqi-companion.jar` + `fabric-api-0.119.4+1.21.4.jar`（都嵌在 exe 里，
  不需要单独下载；实例已有任意 fabric-api 时不重复安装，内容相同不覆盖）
- **自动换皮**：128×128 双层稿按原版 64×64 皮肤空间降采样，进世界即穿上哥特萝莉皮肤；
  头顶「小七」名牌，模型瘦臂（Slim）
- **跟随玩家**：默认跟在身后 2 格、右肩侧 0.9 格，平滑趋近（0.35 格/tick），不瞬移
- **聊天指令**：`#ai 跟随` / `#ai 停下` / `#ai 过来` / `#ai 看我`——本地规则即时生效
- **AI 闲聊**：其它内容（`#ai 你好呀`）转发本地 AI 桥（127.0.0.1 随机端口 + 随机 Bearer token，
  token 只注入本机游戏进程）；**不填 Key 也能用规则兜底**，填了 Key 才走 OpenAI 兼容 LLM
- **登录更省事**：设备码登录的验证代码自动复制到剪贴板——浏览器打开 microsoft.com/link 后直接 Ctrl+V 粘贴
- **任意服务器可用**：小七是客户端幽灵实体（服务端不可见、不参与存档/同步/战斗），
  原版服务器同样能显示，服务器端不需要装任何东西

## 设置

GUI：设置 → 小七伙伴（开关 / 接口地址 / API Key / 模型名）。

```
mc-native-exp config --ai-base-url https://api.deepseek.com/v1 --ai-key sk-... --ai-model deepseek-chat
mc-native-exp config --ai false   # 关闭小七（不装模组、不起 AI 桥）
```

Key 只写本地 `mc-native-exp.config.json`，终端回显与日志一律遮蔽。

## 实测（2026-11-25，Fabric 1.21.4 · 本地单人世界）

| 项 | 结果 |
|---|---|
| 模组加载 | `xiaoqi-companion 1.0.0` + fabric-api 全部模块加载，无 mixin 冲突 |
| 生成 / 换皮 | 进世界自动生成，皮肤/名牌正常（`小七已生成于 (53.5, 63.0, 106.5)`） |
| 跟随 | 前进 2 秒后仍在身后锚点（第三人称确认） |
| `#ai 停下` | 聊天被拦截（不进服务器），回复「好，我在这儿等你。」 |
| `#ai 跟随` | 回复「好，跟着你。」并恢复跟随 |
| `#ai 你好呀` | 本地 AI 桥规则兜底，提示配置 Key（无网络依赖） |
| AI 桥鉴权 | 错误 token → `401 unauthorized`；正确 token → `/hello` 问候 |
| 设备码复制 | 登录时验证代码自动进剪贴板（clip.exe，无换行） |

## 已知限制

- 模组编译目标 1.21.4，其他版本实例不安装（避免破坏既有模组环境）
- AI 只聊天/给建议，不会代替玩家操作游戏
- LLM 需要外网；接口不可达时回落到规则提示，不阻塞游戏

## 上一版（v0.1.0.beta）

目录管理三件套：数据目录可迁移、多版本隔离（`instances\<id>\game`）、游戏目录版本级/全局自定义。
