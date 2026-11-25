# mc-native-exp v0.1.0.beta（实验 · 可能不稳定）

> 版本方案：patch 逐次 +1 至 .9 后进位——0.0.9 → 0.1.0。

## 本版新增：目录管理三件套

- **数据目录可更改**：设置页「📁 选择数据目录」把 versions/libraries/assets/JRE 整体迁到任意磁盘
  （如 D:\MC-Extra）。指针存在 exe 同目录 `mc-native-exp.config.json`，便携语义——整个文件夹搬走即整体迁移；变更需重启生效。
- **多版本隔离**：开关开启后每个版本使用独立游戏目录 `data\instances\<版本>\game`，
  存档 / 模组 / 资源包 / 日志完全隔离，互不影响（实测 Fabric 在隔离目录下正常进引擎）。
- **游戏目录可自定义**：两层——主页每个版本的「📁 目录」按钮设版本级目录（可「↺」恢复跟随全局）；
  设置页设全局默认游戏目录（隔离关闭时生效），可直接指向已有 .minecraft 复用存档模组。

## 命令行

```
mc-native-exp config                        # 查看当前设置
mc-native-exp config --data-dir D:\MC-Extra # 迁移数据目录（重启生效）
mc-native-exp config --isolation true       # 开启多版本隔离
mc-native-exp config --game-dir D:\MC\.minecraft  # 默认游戏目录
```

## 解析优先级

game dir：版本级覆盖 → 隔离模式 instances\<id>\game → 默认游戏目录 → data\game
