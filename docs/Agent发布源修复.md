# Agent 发布源修复

Rust 仓库当前地址为 [YJVIst/YJmonitor](https://github.com/YJVIst/YJmonitor)。旧版主控固定了原账户下的 GitHub 路径，账户更名后会拒绝下载重定向，导致添加服务器时报“Agent 下载或签名校验失败”。

Agent 0.2.3 的 amd64 / arm64 程序、`stable.json` 和 `release-public.txt` 均已发布在 GitHub 的 [v0.11.3](https://github.com/YJVIst/YJmonitor/releases/tag/v0.11.3) 与 [v0.11.5](https://github.com/YJVIst/YJmonitor/releases/tag/v0.11.5)。`stable.json` 包含原始 Ed25519 签名以及两个程序的 SHA-256 和长度。无需开发服务器提供文件。

## 修复已有主控

在主控服务器上以 root 运行：

~~~sh
curl -fL --proto '=https' --proto-redir '=https' --tlsv1.2 \
  https://raw.githubusercontent.com/YJVIst/YJmonitor/main/install.sh \
  -o /root/YJ.sh
bash /root/YJ.sh --repair-agent-source
~~~

脚本自动识别已注册的 Rust 实例，从固定 GitHub Release 下载文件，使用内置原公钥校验签名，再核对版本、大小、哈希与 ELF 架构。全部通过后写入主控数据目录下的 `releases/`，主控接入时再次验签。现有节点、管理员、密钥及服务保持原状，无需重启主控。完成后重新生成之前失败的接入命令。

此修复适用于使用 Agent 0.2.3、支持 `releases/` 缓存的现有 Rust 主控，包括当前 0.11.5。如果有多个实例或使用自定义路径，可下载独立 helper 并指定主控数据目录：

~~~sh
curl -fL --proto '=https' --proto-redir '=https' --tlsv1.2 \
  https://github.com/YJVIst/YJmonitor/releases/download/v0.11.5/agent-release.py \
  -o /root/YJ-agent-release.py
python3 /root/YJ-agent-release.py --state /var/lib/YJ/control
~~~

文件保存在主控私有目录中，以便主控向被控服务器下发安装包；发布源仍为 GitHub。两个架构均会准备。helper 需要 Python 3、OpenSSL 和系统 CA，标准主控安装已包含这些依赖。

## 新安装与离线安装

仓库 main 与 v0.11.5 Release 的维护版 `install.sh` 会自动准备上述 Agent 文件。主控完整 ZIP 和签名清单仍沿用原始签名发行内容。

使用 `--release-dir` 离线安装时，除主控 ZIP 和 `panel-stable.json`，还需将以下文件放在同一 root 所有、其他用户不可写的目录：

- `stable.json`
- `vistart-probe-agent-0.2.3-linux-amd64`
- `vistart-probe-agent-0.2.3-linux-arm64`

签名校验使用脚本内置公钥；`release-public.txt` 供核对，不会替换信任公钥。SHA-256 清单用于检查传输完整性，不能代替签名。

## 维护范围

本次修复更新发布源源码及维护脚本，使用原始签名 Agent 缓存恢复现有主控接入。现有主控二进制未重新编译，其中在线版本检查的旧地址需以后通过签名主控升级修正。
