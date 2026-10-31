<div align="center">

<img src="assets/app-icon.svg" width="96" height="96" alt="CF Best">

# CF Best

**Cloudflare 优选 IP 桌面工作台**

扫描 · 测速 · 可用性验证 · 每日自动优选 —— 一个原生、轻量、无需订阅也能用的 GUI 工具

[![CI](https://github.com/wintopic/CF-BEST/actions/workflows/ci.yml/badge.svg)](https://github.com/wintopic/CF-BEST/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/wintopic/CF-BEST?display_name=tag&sort=semver)](https://github.com/wintopic/CF-BEST/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/wintopic/CF-BEST/total)](https://github.com/wintopic/CF-BEST/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey)
![Rust](https://img.shields.io/badge/rust-1.96%2B-orange?logo=rust)

简体中文 · [English](README.en.md)

<img src="docs/screenshots/daily.png" alt="自动优选" width="900">

</div>

---

## 目录

- [为什么是 CF Best](#为什么是-cf-best)
- [功能](#功能)
- [截图](#截图)
- [下载安装](#下载安装)
- [NAS 与 Linux（网页管理）](#nas-与-linux网页管理)
- [快速上手](#快速上手)
- [锁定出口区域](#锁定出口区域)
- [工作原理](#工作原理)
- [快捷键](#快捷键)
- [数据与隐私](#数据与隐私)
- [从源码构建](#从源码构建)
- [项目结构](#项目结构)
- [参与贡献](#参与贡献)
- [致谢](#致谢)
- [免责声明](#免责声明)
- [许可证](#许可证)

## 为什么是 CF Best

优选 Cloudflare IP 通常要在多个脚本之间来回切换：先扫描、再测速、再逐个验证，最后手动整理结果。CF Best 把这条流程做成了一个桌面应用：

- **一键出结果**：「自动优选」按段池轮转扫描 → 测速 → 验证，直接给出今天最快的 Top N。
- **无需订阅**：扫描、测速和自动优选都不需要节点信息；有 VLESS + WebSocket 节点时才做完整的隧道验证。
- **每天自动跑**：到点运行、错过补跑、托盘常驻、开机自启，放着不管也能每天更新。
- **原生且轻量**：Rust + [GPUI](https://www.gpui.rs/) 自绘界面，单个可执行文件，启动快、占用低，没有 Electron。

## 功能

| 页面 | 作用 |
| --- | --- |
| **自动优选**（启动页） | 一键或每天定时执行「扫描 → 测速 → 验证」，整理并保存 Top N；段池按轮次推进，每轮换一批 IP 段 |
| **扫描 IP** | 从 IP 段、单个 IP、ASN 前缀或推荐段中找出可连通的 Cloudflare 地址，显示机房与握手延迟 |
| **测速排行** | 逐个下载测速（初筛 + 多轮精测），按中位速度排序；附速度分布直方图与「延迟—速度」散点图 |
| **可用性验证** | 用 VLESS + WebSocket 订阅节点做完整隧道验证（订阅可以是 `vless://` 链接或 Clash 配置）；没有节点时做 TLS 基础验证；通过的 IP 可一键导入本机代理软件 |
| **段池优化**（高级） | 切换段池策略（均衡轮转 / 网络优选 / 顺序覆盖），调整起点，粗测本机网络下各段的表现；按本机网络划分各段的出口区域 |
| **历史记录**（高级） | 查看历次运行；对比最近两次测速的 Top 10 排名与速度变化 |

更多细节：

- 🌏 **锁定出口区域**：按本机网络实际落到的 Cloudflare 机房，把段池划分到国家 / 地区（香港、日本、美国……）。锁定后，自动优选只扫描落在这些区域的段，结果也只保留这些区域的 IP；也可以填机房代码（如 `SJC`、`NRT`）精确锁定。结果表显示机房、城市与国家，扫描结果可按区域筛选后再送去测速。
- 🔗 **手动三步一键衔接**：「下一步：测速」「下一步：验证」会载入结果并直接开始下一步。
- 💾 **结果自动恢复**：最近一次的扫描、测速与验证结果在重启后自动恢复。
- 📋 **复制与导出**：点击行复制 `IP:端口`，Top N 一键「复制全部」；导出 TXT / CSV（Excel 可直接打开）。
- 📥 **导入代理软件**：隧道验证通过后，把优选 IP 套到订阅节点上，一键导入本机的 Clash Verge、Karing、Stash、FlClash 等客户端；也可复制本机订阅地址或节点链接。客户端更新订阅即可拿到最新一轮自动优选的结果。
- 🔄 **检查更新**：启动时与每天自动检查（可关闭），有新版本时状态栏右下角提示。一键更新会同时测速 GitHub 直连与 gh-proxy.com 等国内镜像，走最快的线路下载，并用 GitHub 给出的 SHA-256 校验；Windows 自动替换并重启，macOS 打开安装包。
- ⏰ **可靠的定时运行**：到点后当天还没跑过就开始，错过时间会补跑，有任务时排队等待，后台运行不打断当前操作。
- 🎨 **主题**：森林 / 薄荷 / 蜜蜂 / 羊皮纸 4 种风格 × 浅色 / 深色 / 跟随系统；界面缩放 90%–125%。
- ⌨️ **全键盘操作**：页面切换、开始 / 停止任务、焦点移动都有快捷键。
- 🪟 **Windows 集成**：托盘常驻与通知、任务栏进度、开机自启到托盘。

## 截图

| 扫描 IP | 测速排行 |
| :---: | :---: |
| <img src="docs/screenshots/scan.png" alt="扫描 IP"> | <img src="docs/screenshots/speed.png" alt="测速排行"> |
| **可用性验证** | **深色主题** |
| <img src="docs/screenshots/verify.png" alt="可用性验证"> | <img src="docs/screenshots/daily-dark.png" alt="深色主题"> |

## 下载安装

前往 [**Releases**](https://github.com/wintopic/CF-BEST/releases/latest) 下载最新版本：

| 平台 | 文件 | 说明 |
| --- | --- | --- |
| Windows 10 / 11（x64） | `CFBest-<版本>-windows-x64.exe` | 免安装，双击运行 |
| macOS 11+（Apple 芯片 / Intel） | `CFBest-<版本>-macos-universal.dmg` | 打开后把 **CF Best** 拖入「应用程序」 |
| Linux 服务器（x64 / ARM64） | `cfbest-cli-<版本>-linux-x64` / `-linux-arm64` | 无窗口版，浏览器管理，见 [NAS 与 Linux](#nas-与-linux网页管理) |
| NAS（Docker） | 镜像 `ghcr.io/wintopic/cf-best`，或 `cf-best-docker-<版本>-linux-*.tar.gz` | 群晖、极空间、飞牛等，见 [NAS 与 Linux](#nas-与-linux网页管理) |

每个版本都附带 `SHA256SUMS.txt`，可用于校验下载文件。

<details>
<summary><b>macOS 首次打开提示「无法验证开发者」？</b></summary>

安装包没有使用 Apple 开发者证书签名（仅做了 ad-hoc 签名）。首次打开时在 Finder 中**右键点击 CF Best → 打开**，或在终端执行：

```bash
xattr -dr com.apple.quarantine "/Applications/CF Best.app"
```

</details>

<details>
<summary><b>Windows 提示「Windows 已保护你的电脑」？</b></summary>

程序没有代码签名证书，SmartScreen 会对新发布的文件给出提示。点击「更多信息」→「仍要运行」即可。可以先用 `SHA256SUMS.txt` 校验文件。

</details>

## NAS 与 Linux（网页管理）

在 NAS 或 Linux 服务器上常驻运行无窗口版，用浏览器管理：每天定时自动优选，局域网里的手机、电脑直接在代理软件里添加它提供的订阅地址，每轮优选完成后更新订阅即可拿到最新节点，不必开着电脑。

网页里可以立即运行 / 停止、设置每日运行时间与优选参数、填写 VLESS 订阅链接、查看 Top 结果、历史与实时日志，并复制订阅地址（Clash / V2Ray 两种格式，或按客户端自动选择）。

### Docker（推荐）

```bash
docker run -d --name cf-best --restart unless-stopped   -p 27208:27208 -v ./cf-best:/data   -e TZ=Asia/Shanghai -e CFBEST_PASSWORD=change-me   ghcr.io/wintopic/cf-best:latest
```

或使用仓库里的 [`docker-compose.yml`](docker-compose.yml)：`docker compose up -d`。然后在浏览器打开 `http://<NAS 的 IP>:27208`。镜像同时提供 x64 与 ARM64。

<details>
<summary><b>群晖 / 极空间 / 飞牛等 NAS 的图形界面设置</b></summary>

在 NAS 的 Docker 应用（群晖 Container Manager、极空间 Docker、飞牛 fnOS Docker 等）里：

1. **镜像**：搜索或添加 `ghcr.io/wintopic/cf-best:latest`。拉取不到（网络连不上 ghcr.io）时，从 [Releases](https://github.com/wintopic/CF-BEST/releases/latest) 下载 `cf-best-docker-<版本>-linux-x64.tar.gz`（ARM 机型选 `-linux-arm64`），在镜像管理里选择「导入」。
2. **端口**：容器端口 `27208` 映射到本机任意空闲端口（例如同样的 `27208`）。
3. **存储空间 / 卷**：把 NAS 上的一个文件夹（如 `/docker/cf-best`）挂载到容器的 `/data`，设置与结果都保存在这里。
4. **环境变量**（可选）：`CFBEST_PASSWORD` 设置网页访问密码；`TZ` 默认 `Asia/Shanghai`。
5. 勾选「自动重启」，启动后用 `http://<NAS 的 IP>:<端口>` 打开网页。

网络模式用默认的 bridge 即可，不需要 host 模式或特权。

</details>

### Linux 可执行文件

从 Releases 下载对应架构的 `cfbest-cli-<版本>-linux-*`（静态链接，任何发行版都能运行）：

```bash
chmod +x cfbest-cli-*-linux-x64
./cfbest-cli-*-linux-x64 serve
```

数据保存在 `~/.local/share/CFBest`（或 `CFBEST_DATA_DIR` 指定的目录）。用 systemd 常驻时，`ExecStart` 写成 `/usr/local/bin/cfbest-cli serve` 即可。

### 选项与安全

| 环境变量 | 命令行 | 说明 |
| --- | --- | --- |
| `CFBEST_PASSWORD` | `--password` | 网页访问密码，默认不设。**NAS 暴露到公网时务必设置** |
| `CFBEST_LISTEN` | `--listen` | 监听地址，默认 `0.0.0.0:27208` |
| `CFBEST_ALLOWED_HOSTS` | `--allow-host` | 未设密码时额外允许的访问域名（逗号分隔） |
| `CFBEST_DATA_DIR` | | 数据目录，镜像中为 `/data` |
| `CFBEST_SUB_URL` | | 启动时写入的 VLESS 订阅链接，之后也可在网页上修改 |
| `TZ` | | 时区，决定每日定时的时间 |

- 订阅地址形如 `http://<IP>:27208/<令牌>/daily`，令牌随机生成、即为凭证，不需要密码，代理软件可以直接更新；网页上可随时「更换令牌」让旧地址失效。
- 没设密码时，网页只接受用 IP、机器名或 `.local` / `.lan` 等内网域名访问，防止外部网页借 DNS 重绑定操作服务；用自己的域名访问请设置密码，或把域名加入 `CFBEST_ALLOWED_HOSTS`。
- 与图形界面共用数据格式：可以把电脑上的 `CFBest` 数据目录复制到 NAS 的挂载目录里，沿用段池、设置与订阅。同一个数据目录同一时间只能被一个实例使用。

## 快速上手

**只想要几个快的 IP？** 打开应用，在「自动优选」点 **立即运行一次**。几分钟后 Top N 就会出现在下方，点 **复制全部** 即可使用。

**想每天自动更新？** 打开 **每天自动运行** 并设定时间。应用会同时开启托盘常驻（Windows），关闭窗口后在托盘继续运行；再打开 **开机自启**，重启电脑后也会自动恢复。

**想手动控制每一步？**

1. **扫描 IP**：粘贴 IP 段（如 `104.16.0.0/24`），或点输入框右下角的推荐段，然后点 **开始扫描**。
2. **测速排行**：在扫描结果上点 **下一步：测速**，会按握手延迟取前 20 个逐个测速。
3. **可用性验证**：在测速结果上点 **下一步：验证**。如需完整的隧道验证，先在「订阅设置」中填入订阅链接；验证通过后，在结果下方的「导入代理软件」中点本机的客户端即可导入。

> 高级模式（侧栏底部开关或 `Ctrl+Shift+A`）会显示全部参数、段池优化与历史记录。简单模式与高级模式共用同一份参数，默认即推荐值。

## 锁定出口区域

Cloudflare 的地址是任播的：同一个 IP 段，从不同的网络连过去会落到不同的机房，GeoIP 数据库也大多把它们标成美国，所以「这个段在哪个国家」只能在本机实测。CF Best 对段池里的每个 /24 段探测 `/cdn-cgi/trace`，记下它落到的机房（如 `HKG`、`NRT`、`LAX`），再换算成国家 / 地区。Workers / Pages 搭建的节点就在这个机房运行，所以它也是代理的出口所在地。

1. 在「段池优化」页的「出口区域」卡片点「划分新段」（首次锁定区域时也会自动划分一次）。整段探测，几百个段需要几分钟，可随时停止，已探测的段会保留。
2. 在「自动优选」页的「出口区域」里点选国家 / 地区，或在下方填写其他国家 / 地区代码（`SG`）或机房代码（`SJC`）。
3. 之后的自动优选只在落在这些区域的段中轮转（有单独的进度，不影响不锁定时的轮转），测速、验证后只保留出口在锁定区域的 IP。

每轮优选整段扫描过的段会顺便更新划分，手动扫描的命中也会补充进来；换了网络（比如换了运营商）后可以「重新划分全部」。无窗口版的网页上有同样的设置，命令行可用 `--region HK,JP` 临时指定。

## 工作原理

```mermaid
flowchart LR
    A[段池<br/>AS906 前缀拆分为 /24] -->|本轮取 N 段| B[扫描<br/>TLS 握手 + /cdn-cgi/trace]
    B -->|按握手延迟取前 M 个| C[测速<br/>speed.cloudflare.com 逐个下载]
    C -->|最快的 2 × Top N| D{有 VLESS + WS 节点?}
    D -->|是| E[隧道验证<br/>WS 升级 → VLESS → HTTP 204]
    D -->|否| F[TLS 基础验证<br/>TLS 握手 + HTTP 回源]
    E --> G[Top N<br/>保存、导出、复制]
    F --> G
```

- **扫描**：并发连接每个 `IP:端口`，以 Cloudflare 的 SNI 完成 TLS 握手，并请求 `/cdn-cgi/trace` 读取机房代码（colo）。能返回 colo 的地址记为命中。
- **测速**：逐个串行测速，避免多个连接互相分摊带宽。每个候选先短时初筛，最快的若干个再做多轮精测，按中位速度排序。遇到 Cloudflare 限流（HTTP 429）会提示并适时停止。
- **验证**：
  - **隧道验证**：加载 VLESS + WebSocket 节点后，经该 IP 建立 WebSocket 隧道，通过 VLESS 访问 `cp.cloudflare.com/generate_204`，三步都成功才算通过。
  - **TLS 基础验证**：没有节点时，只检查 TLS 连接与 HTTP 回源。通过不代表代理节点可用，结果和导出文件里都会标明验证类型。
- **段池**：自动优选从 RIPE 拉取 AS906 的 IPv4 前缀并拆成 /24 段，每轮取连续 N 段，下一轮从后面继续，跑完一圈后开始新一轮。也可以按本机粗测结果排序（网络优选）。

## 快捷键

macOS 上用 `⌘` 代替 `Ctrl`。

| 快捷键 | 动作 |
| --- | --- |
| `Ctrl+1` ~ `Ctrl+4` | 自动优选 / 扫描 IP / 测速排行 / 可用性验证 |
| `Ctrl+5` / `Ctrl+6` | 段池优化 / 历史记录（高级模式） |
| `Ctrl+Enter` | 开始当前页面的任务 |
| `Ctrl+.` | 停止正在运行的任务 |
| `Ctrl+Shift+A` | 切换高级模式 |
| `Ctrl+B` | 收起 / 展开侧栏 |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | 放大 / 缩小 / 重置界面缩放 |
| `Tab` / `Shift+Tab` | 在按钮、导航与输入框之间移动焦点 |

## 数据与隐私

- 所有设置、历史记录与导出文件都保存在本机：
  - Windows：`%APPDATA%\CFBest`
  - macOS：`~/Library/Application Support/CFBest`
  - Linux：`~/.local/share/CFBest`；Docker 镜像中为挂载的 `/data`
  
  侧栏底部的「数据目录」可以直接打开该文件夹。
- 应用只访问以下地址：被测 IP 本身、Cloudflare 的公开接口（IP 段列表、`/cdn-cgi/trace`、`speed.cloudflare.com`、`cp.cloudflare.com`）、RIPE Stat（查询 ASN 前缀）、你填写的订阅链接，以及检查与下载更新用的 GitHub 和下载镜像（gh-proxy.com、ghfast.top、ghproxy.net）。
- 导入过代理软件后，应用会在 `127.0.0.1` 上提供订阅地址供客户端更新（只接受本机访问，地址含随机令牌）；应用没在运行时，客户端保留上次导入的节点。
- 无窗口版的局域网服务（`cfbest-cli serve`）在局域网端口上提供网页与订阅地址，安全设置见 [NAS 与 Linux](#nas-与-linux网页管理)。
- 没有统计或遥测。检查更新只向 GitHub 查询最新版本信息，不上传任何数据，可在状态栏右下角的版本号弹层里关闭。订阅链接与节点信息只保存在本机：链接在 `settings.json`，最近一次成功载入的节点在 `subscription.json`（订阅拉取失败时沿用）。

## 从源码构建

需要 [Rust](https://rustup.rs/) 1.96+（edition 2024）。

```bash
git clone https://github.com/wintopic/CF-BEST.git
cd CF-BEST
cargo run --release
```

- **Windows**：需要 Visual Studio 的 MSVC 生成工具（“使用 C++ 的桌面开发”）。产物为 `target/release/CFBest.exe`，已内嵌图标与版本信息。
- **macOS**：需要 Xcode 命令行工具（`xcode-select --install`）。`.app` 与 `.dmg` 的打包步骤见 [`release.yml`](.github/workflows/release.yml)。
- **Linux / 无窗口版**：`cargo build --release --no-default-features --bin cfbest-cli` 只编译命令行与局域网服务，不需要图形界面依赖；`docker build -t cf-best .` 构建镜像。

提交前检查（与 CI 一致）：

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo clippy --locked --no-default-features --all-targets -- -D warnings   # 无窗口版
cargo test --locked -- --ignored   # 需要联网的测试：边缘 trace、测速下载、RIPE 前缀等
```

<details>
<summary><b>调试用环境变量（仅 debug 构建）</b></summary>

使用任一变量时，本次运行不会写入设置。

| 变量 | 作用 |
| --- | --- |
| `CFBEST_THEME=bee-light` | 启动主题 |
| `CFBEST_PAGE=speed` | 启动页面（scan / speed / verify / daily / pool / history） |
| `CFBEST_SCALE=1.25` | 界面缩放 |
| `CFBEST_SIDEBAR=collapsed` | 收起侧栏 |
| `CFBEST_TRAY=1` | 开启托盘常驻 |
| `CFBEST_DEMO=1` | 载入内存中的演示数据 |
| `CFBEST_TRAY_SELFTEST=1` | 托盘自检，结果写入运行日志 |
| `CFBEST_UPDATE_POPOVER=1` | 启动时打开检查更新弹层 |
| `CFBEST_FAKE_VERSION=0.0.9` | 冒充旧版本，用来测试更新流程 |
| `CFBEST_AUTO_UPDATE=1` | 发现新版本后自动下载并安装（配合上一项，在程序副本上测试替换与重启） |

</details>

## 无窗口模式（cfbest-cli）

除图形界面外，仓库还带一个无窗口的命令行入口。`cfbest-cli serve` 常驻运行局域网服务（见 [NAS 与 Linux](#nas-与-linux网页管理)）；不带 `serve` 时运行一轮优选后退出，适合用计划任务 / launchd / systemd 定时跑：

```bash
cargo run --release --no-default-features --bin cfbest-cli -- --segments 20 --test 20 --top 5
```

它与图形界面共用同一份数据目录（设置、段池、订阅缓存、每日进度），两个入口可以交替使用：界面里配好订阅链接，命令行就能直接跑隧道验证。

| 选项 | 说明 |
| --- | --- |
| `--segments <N>` | 本轮扫描的 /24 段数（默认取设置） |
| `--test <N>` | 进入测速的 IP 数（默认取设置） |
| `--top <N>` | 最终保留的 Top N（默认取设置） |
| `--concurrency <N>` | 扫描并发（默认取设置） |
| `--out <PATH>` | 结果另存到文件 |
| `--json` | 以 JSON 输出到标准输出 |
| `--quiet` | 不打印过程日志 |
| `--reset` | 忽略上次进度，从段池第一段重新开始 |

Top 结果写标准输出、过程日志写标准错误，便于重定向与管道：

```bash
cfbest-cli --quiet > top.txt 2>> run.log
cfbest-cli --json --quiet | jq '.top[0]'
```

退出码：`0` 有结果，`3` 本轮无可用 IP，`1` 运行出错，`2` 参数错误。

## 项目结构

```
src/
  main.rs            窗口创建、快捷键与菜单
  bin/cfbest-cli.rs  无窗口的命令行入口，复用 core 的全部逻辑
  server/            局域网服务（cfbest-cli serve）：定时优选、网页管理界面（index.html）与订阅地址
  core/              与界面无关的业务逻辑：扫描、测速、验证、段池、每日优选、存储、统计
  app/               应用状态与各页面：侧栏、标题栏、事件处理、操作回调、*_page.rs
  ui/                通用组件：按钮、输入框、提示气泡、图表
  jobs.rs            后台任务调度（tokio）与事件
  theme.rs           主题、界面缩放与设计令牌
  theme_tokens.rs    各主题配色数值（源自 Qoder Design Kit）
  tray.rs            系统托盘（Windows）
  taskbar.rs         任务栏进度（Windows）
  autostart.rs       开机自启（Windows 注册表 / macOS LaunchAgent）
assets/              界面图标（SVG）与应用图标
docs/screenshots/    README 截图
.github/workflows/   CI 与发布流程
```

## 参与贡献

欢迎提交 Issue 和 Pull Request！开始之前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。

- 🐛 [报告问题](https://github.com/wintopic/CF-BEST/issues/new?template=bug_report.yml)
- 💡 [功能建议](https://github.com/wintopic/CF-BEST/issues/new?template=feature_request.yml)

## 致谢

- [GPUI](https://www.gpui.rs/)：Zed 团队的 GPU 加速 UI 框架
- [tokio](https://tokio.rs/)、[rustls](https://github.com/rustls/rustls)：异步运行时与 TLS
- 应用图标的云朵图形来自 [Font Awesome Free](https://fontawesome.com)（[CC BY 4.0](https://fontawesome.com/license/free)）
- 界面布局与配色参考了 Qoder Design Kit

## 免责声明

本项目仅用于网络连通性与性能测试的学习和研究。请遵守所在地法律法规以及 Cloudflare 的服务条款，不要用于任何违法用途。测速会产生下载流量，请合理设置频率。

本项目与 Cloudflare, Inc. 无任何关联，也未获其认可。Cloudflare 及其标志是 Cloudflare, Inc. 的商标。

## 许可证

[MIT](LICENSE) © 2026 AIGCFREE
