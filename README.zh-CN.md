# Guitar Girl Fan Memorial Server

[English](README.md) · 简体中文

**Welcome home, Lily!** 为非官方《吉他少女》纪念版提供内建 Rust 服务端：在本机运行玩法逻辑，保存互相隔离的存档，不需要持续运行的远程后端。

本仓库构建的是服务端组件，**不是可安装的游戏**。玩家请先阅读 [Patcher 使用说明](https://github.com/guitar-girl-resuscitation/guitar-girl-memorial-patcher/blob/main/README.zh-CN.md)。

## 三个仓库分别做什么

| 仓库 | 职责 |
| --- | --- |
| [Server](https://github.com/guitar-girl-resuscitation/guitar-girl-memorial-server) | Rust 玩法、协议、SQLite 存档与 Android 内建服务端库 |
| [Patch](https://github.com/guitar-girl-resuscitation/guitar-girl-memorial-patch) | 自有客户端集成、纪念版界面、身份隔离与有校验的变换规则 |
| [Patcher](https://github.com/guitar-girl-resuscitation/guitar-girl-memorial-patcher) | CLI / 网页打包、原包验证、资源提取、签名和下载 |

游戏运行时，修改后的 Unity 客户端通过带认证的本机回环连接内建 Rust 服务端。补丁网站**不是游戏服务器**，游玩时不需要它持续在线。

## 架构

```text
Unity 客户端 → Patch → 带认证的本机 HTTP / Thrift
                         ↓
                     Rust handlers
                     ├─ SQLite：按 USN 隔离的玩家状态
                     ├─ master.sqlite：从用户原包提取
                     └─ Android AssetManager：原包已有资源
```

| Crate | 职责 |
| --- | --- |
| `protocol` | 类型化线型契约、封包和编解码 |
| `domain` | 玩法模型与显式业务行为 |
| `persistence-sqlite` | 事务、迁移、身份隔离与恢复 |
| `master-data` | 读取由客户端生成的只读主表 |
| `policy-memorial` | 版本化纪念版规则 |
| `transport-http` | 本地 HTTP 与游戏 / PMang 兼容路由 |
| `android-ffi` | 客户端启动器调用的稳定 C ABI |
| `server-cli` | 桌面协议调试宿主 |
| `contract-tests` | 线型和行为回归测试 |

### 存档和时间约束

- 每个槽位拥有不可变的正整数 USN（`U_seq`）及数字字符串 `U_id`；玩家数据行按 USN 隔离。
- 登录会话绑定身份，拒绝身份不符或过期序号的写入。
- 购买、领奖、邮件发放/领取采用事务和幂等键，数据库提交后才返回成功。
- 切档只记录待切换目标；当前 Unity 会话结束后，才能激活新身份及其客户端键命名空间。
- SQLite 使用 WAL、外键和持久化写入；关服快照与恢复机制详见架构文档。
- 倒计时保存绝对时间，日历使用设备 Unix 时间及 UTC 偏移，不按服务端重启次数计算。关游戏后不需要后台常驻。

支持从全新纪念版存档开始，不应假设旧实验存档或官方云存档可以直接导入。

## 纪念版规则

以 [版本化 policy](policy/memorial-policy.json) 为准；Server 与 Patch 共享规则并校验指纹。

- 指定升级/粉丝要求降低至 0.1 倍，保留粉丝第一级及技能解锁等级要求。
- 成就仅降低“粉丝俱乐部”和累计获取赞的要求，不缩减签到天数或全部成就计数。
- 不降低 CH1 / CH2 实际赞产量和 CH2 音符产量。
- 糖果、巧克力升级按递增曲线计费，根据原始价格类别，满级费用上限为 10～20。
- 可购买服装/吉他在原货币下定价 10；赞解锁项遵循养成规则。剧情、通行证与绝版奖励保留各自来源。
- CH1 非默认服装/吉他增益为 30% / 20%；CH2 使用各类别原版最高增益。
- 技能冷却 0.1 倍，技能重置五分钟；安可基础一小时，再按等级降低。
- 好感要求保留原版，CH3 奖励加速；体力上限 200，每秒恢复 1 点。
- 通行证按设备本地日期每日轮换，手动选期重设该档锚点；免费、星级奖励分别记录领取。
- 绝版物品一封邮件一个，已拥有或已在待领邮件中的唯一物品拒绝重复发放。

## 构建与测试

建议使用 CI 固定的工具链（当前 Rust 1.96.0）、Python 3.11+ 和 Git。内存有限时保持低编译并发。

```sh
git clone https://github.com/guitar-girl-resuscitation/guitar-girl-memorial-server
cd guitar-girl-memorial-server
cargo test --workspace --locked -j2
cargo build --locked --release -j2 -p ggfm-server
```

### 桌面调试服务端

提供 Patcher 从受支持原包生成的 `master.sqlite`；它不会随公开仓库提供。

```sh
export GGFM_MASTER_SQLITE=/absolute/private/master.sqlite
export GGFM_DATABASE=/absolute/private/ggfm.sqlite3
# 可选：通过环境变量注入一次性的本机会话令牌。
export RUST_LOG=info
./target/release/ggfm-server
```

PowerShell 对应写法为 `$env:GGFM_MASTER_SQLITE = 'C:/private/master.sqlite'`，执行文件为 `.\target\release\ggfm-server.exe`。

CLI 输出动态分配的本机端口，Ctrl-C 退出。桌面调试时钟采用 UTC，也不提供 Android AssetManager 资源，因此不是独立完整的 Android 游戏宿主；Android 端由启动器传入设备时间和资源访问能力。

### Android 库

安装 NDK 27.3.13750724 及 Rust Android target：

```sh
rustup target add aarch64-linux-android
python tools/build_android.py --ndk "$ANDROID_HOME/ndk/27.3.13750724"
```

Android Release 提供 `libggfm_server.so`。稳定 ABI 见 [include/ggfm_server.h](include/ggfm_server.h)，包括启动、端点、登录、日志读取和关闭操作。Patch 与 Server 必须同时匹配 ABI 和 policy 指纹。

## Release 与组件配对

[Releases](https://github.com/guitar-girl-resuscitation/guitar-girl-memorial-server/releases) 提供编译组件及 SHA-256 校验文件。main 构建成功后替换唯一的 `nightly` Release；版本标签发布对应版本。Nightly 会变化，部署时必须记录准确源码提交及产物摘要。

应使用选定 Patch Release 的 `dependencies.json` 指定的 Server，不要分别下载两个不断变化的 Nightly 后直接混用。Patcher 消费预编译组件，不会为每个访客现场构建 Rust 服务端。

## 进一步阅读

- [架构与存档约束](docs/ARCHITECTURE.md)
- [协议契约与证据边界](docs/PROTOCOL.md)
- [奖励消耗契约](docs/CONSUME_REWARD_CONTRACT.md)
- [发布流程](docs/RELEASING.md)

## 项目边界、贡献与许可

这是非官方的粉丝纪念版与互操作项目，与原开发商、发行商没有官方关联或背书。不恢复官方账号、云存档、支付或已退役的线上服务。部分历史服务端专有数值采用纪念版兼容值，不宣称完整复现原服全部数据。

项目代码采用 [AGPL-3.0-or-later](LICENSE)，第三方组件保留各自许可；该许可不覆盖原游戏。请只使用你有权使用的原始安装包。

请勿提交 APK/XAPK、AssetBundle、原版 DEX/IL2CPP 二进制、完整反编译导出、抓取的专有主表、私人存档或签名密钥。反馈问题请提供组件版本/提交、章节、复现步骤和脱敏诊断日志。修改行为时补充契约/回归测试，并同步维护两种语言的 README。
