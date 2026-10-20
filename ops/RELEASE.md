# 维护者发布流程

普通使用者直接下载完整安装包，不需要以下工具或签名私钥。

1. 修改主控 Cargo.toml / Cargo.lock 版本、界面版本、安装器版本、固定 Release URL 和下载跳转白名单。Agent 若变更，单独更新其版本及主控允许的 Agent 版本。
2. 按 BUILD.md 编译两个架构，保留 Cargo.lock；把主控放入 bin/，Agent 放到独立发布目录。
3. 完成单元测试、依赖审计、隔离安装、SSH/SFTP、下载校验和敏感信息扫描。上游已知告警必须写入发布说明。
4. 使用 Python 3 和 cryptography 运行 ops/package-release.py。签名密钥为原始 Ed25519 32 字节种子或 64 字节私钥，必须在源码目录之外且权限为 0600。工具校验其公钥与主控内置公钥相同。
5. 发布目录包含完整安装 ZIP、校验和、panel-stable.json、controller-stable.json 和主控二进制。另放 Agent 二进制及以同一公钥验证的 stable.json。未变更的 Agent 可以复用已验签的原文件。
6. 在 YJVIst/YJmonitor 创建 vX.Y.Z 版本标签并上传源码；将全部资产上传到对应 GitHub Release，逐一核对名称、长度与哈希。签名密钥永远不进入 Git、安装包、Actions 日志或 Release。
7. 从无 GitHub 登录态的环境验证公开下载，在支持的全新 Linux 系统 上按 README 命令安装。验证完成后公布正式发行说明。

Git 源码树不提交 bin/、运行状态、编译缓存和安装 ZIP。完整安装 ZIP 中的 storage 为空，默认不预设管理员账户或节点。仓库中的第三方许可证保持上游原文。

维护者若 Fork 到不同仓库，需要同时更新安装脚本与主控的发布地址、公钥、版本和 GitHub 跳转白名单，再编译并签署自己仓库的发行文件。不能只改 README 下载链接。

## 2026-10-20 Agent 发布源维护

仓库账户现为 YJVIst。旧版主控拒绝 coexacx → YJVIst 的跨仓库路径重定向。Agent 0.2.3 的两个架构、stable.json（含 Ed25519 签名）及 release-public.txt 已存在于 v0.11.3 / v0.11.5，保留原始内容。

本次通过 `ops/agent-release.py` 与安装器的 `--repair-agent-source` 下载并校验原发布，写入现有主控支持的私有 releases 缓存。安装器的普通安装路径同样准备此缓存；离线安装需提供两份 Agent 与 stable.json。

本次维护更新仓库源码、安装引导脚本与独立修复脚本，未重新构建主控或改写签名 ZIP。源码中的新仓库地址与过期 Agent 缓存处理随以后正式编译发布生效。现有主控的在线版本检查仍可能因旧地址失败，需要后续签名主控升级；Agent 接入可独立修复。后续发布变更 Agent 版本时，需同步维护安装器内嵌的 helper、固定版本和发布资产。
