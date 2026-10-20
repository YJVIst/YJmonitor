# Rust 源码构建

普通宝塔安装使用完整 Release 安装包 bin/ 中的静态二进制（Git 源码树不携带二进制），无需 Rust、Cargo、Go、Node.js 或数据库。主控与 Agent 均为 Rust。本仓库以 -web 启动主控，由 Nginx 提供 TLS/WSS，不安装 PHP-FPM。

## 开发构建

在 Debian/Ubuntu 构建机安装 rust-toolchain.toml 指定的 Rust 工具链、musl-tools、gcc-aarch64-linux-gnu、对应交叉 libc 头文件。通过 rustup 安装 x86_64-unknown-linux-musl 和 aarch64-unknown-linux-musl 目标，再在项目根目录执行：

```sh
./ops/build-backend.sh ./build
```

构建使用 Cargo.lock 锁定依赖，先运行格式检查、单元测试和 Clippy，再构建两种架构。正式版不要启用 `test-deployment`；该功能只为隔离测试服务改名，避免与生产 Agent 冲突。默认输出 `probe-linux-amd64`、`probe-linux-arm64` 与两个带版本号的 Agent。

Nginx + Rust 发行版使用相同主控构建，-web 启用嵌入页面与网页安装，--install 从标准输入读取 JSON 完成 CLI 安装。构建前先生成前端资源并同步 app/view.html 与 public/assets，build.rs 会将它们编入主控二进制。

本版使用控制器 CLI：`-state`、`-listen`、`-origin`、`-web`。内部监听强制回环地址。`--version` 显示组件版本。

## 依赖扫描

分别在 `source/agent-rust` 和 `source/controller-rust` 执行 `cargo audit --json`，保留原始输出。主控 RSA 依赖的上游告警与可达性判断见 `docs/RUST-SECURITY.md`；不要过滤扫描输出或把已知告警称为零告警。发现新的告警需重新评审再发布。

## 前端

```sh
cd source/frontend-tools
npm ci --ignore-scripts
cd ..
node build.mjs
```

将生成的 `source/public/assets/` 复制到项目 `public/assets/`，将 `source/public/index.html` 复制为 `app/view.html`。先发布带内容哈希的资源，再替换模板。国家数据位于 `source/controller-rust/assets/countries.json`，旗帜与终端组件均自托管。

## Agent 发布源

主控从 https://github.com/YJVIst/YJmonitor/releases/download/v0.11.3/ 获取 `stable.json`，先用内嵌 Ed25519 公钥验证签名，再验证精确版本、架构、文件名、长度与 SHA-256。校验失败会终止部署，不执行远端二进制。

发布下载仅允许固定仓库 HTTPS 与 GitHub 资产 CDN，最多跟随 4 次重定向。其他 HTTP 客户端仍不跟随重定向。

自建发布源时，同时修改安装器和主控的下载地址、跳转白名单及版本；修改 `source/controller-rust/src/deploy.rs` 的地址，替换 `assets/release-public.txt` 的公钥，并用自己的离线私钥签署发布清单。签名私钥不得放入源码包或网站目录。普通使用者无需自己编译或签名。

主题时间轴与终端调色板测试：

    node --test source/tests/seasons.test.mjs

本仓库使用 ops/package-release.py 生成唯一的 Rust 发行包，默认 --variant rust。签名私钥必须放在源码树之外。正式标签为 v0.11.4，在 YJVIst/YJmonitor 中发布为 latest；不操作原 PHP 仓库的 latest。

0.11.4 没有修改 Agent（仍为 0.2.3），主控继续使用 v0.11.3 的固定已签名 Agent 下载源。0.11.4 Release 同时附带相同 Agent 二进制；不要删除该固定源，变更 Agent 时一起更新下载地址与验签版本。
