# mc-native-exp {{VERSION}}（实验 · 可能不稳定）

## 功能

- **现代化 GUI**：六标签页（主页启动 / 登录 / 加载器 / 内容库 / 原版 / 设置），
  科幻 HUD 风格（深空底 + 青紫辉光按钮 + 呼吸光效 + HUD 分隔线），logo 内嵌 exe 图标
- **内容库（Modrinth）**：模组 / 材质包 / 光影 / 地图 / 插件 搜索与一键安装
- **原版下载**：官方 piston-meta 全版本（925 个）全套下载（client/libraries/assets/natives）
- **四大加载器自动安装**：fabric / quilt / neoforge / forge（Fabric/Forge/Quilt 已实测启动进入引擎初始化）
- **微软正版认证**：设备码流程 + 所有权校验（无正版拒绝继续）
- **启动优化**：BMCLAPI 镜像优先、Range 断点续传、SHA1/Content-Length 双校验、G1 参数预设
- **CLI 完整保留**：所有子命令与 GUI 并存（`menu` = 控制台交互菜单）
