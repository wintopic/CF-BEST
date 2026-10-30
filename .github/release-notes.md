{{CHANGES}}

## 下载

| 系统 | 文件 |
| --- | --- |
| Windows 10 / 11（x64） | `CFBest-{{VERSION}}-windows-x64.exe`，免安装，双击运行 |
| macOS 11+（Apple 芯片 / Intel） | `CFBest-{{VERSION}}-macos-universal.dmg`，打开后把 CF Best 拖入「应用程序」 |
| Linux 服务器（x64 / ARM64） | `cfbest-cli-{{VERSION}}-linux-x64` / `-linux-arm64`，无窗口版，`chmod +x` 后运行 `./cfbest-cli-* serve`，浏览器打开 `http://<IP>:27208` |
| NAS（群晖、极空间、飞牛等 Docker） | 镜像 `ghcr.io/wintopic/cf-best:{{VERSION}}`；拉取不到时下载 `cf-best-docker-{{VERSION}}-linux-x64.tar.gz`（ARM 机型选 `-linux-arm64`）在 Docker 管理界面导入 |

**首次运行提示**

- Windows 提示「Windows 已保护你的电脑」：点击「更多信息」→「仍要运行」。
- macOS 提示「无法验证开发者」：在 Finder 中右键点击 CF Best →「打开」。

下载后可用 `SHA256SUMS.txt` 校验文件。
