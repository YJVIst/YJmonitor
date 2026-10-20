# Nginx + Rust 部署教程（0.11.5）

本发行版将页面、静态资源、网页安装向导、API、WSS 和业务主控放在一个 Rust 可执行文件里。运行时使用 Nginx 与一个主控进程，不启动 PHP-FPM，不需要数据库、Node.js 或 Rust 编译环境。源码包同时附带 amd64、arm64 二进制，实际只运行对应架构的一个。

本仓库为独立 Nginx + Rust 项目，发布与更新均使用 YJVIst/YJmonitor 的 v* 标签。原 PHP 项目保留在 coexacx/yuji-probe。Agent 0.2.3、业务协议和私有状态格式兼容。

## 一、一键部署

按照 [Linux 一键安装与管理菜单](Linux一键安装.md) 下载并执行当前安装脚本。只需填写站点名称，结束显示 IP:端口和自动生成的管理员密码。

安装器不安装 Nginx、不申请证书、不占用 80/443；可在已有宝塔或网站的主机上安装。安装后可选菜单 13 [自动配置 Nginx + HTTPS](自动HTTPS.md)，也可继续按 [反向代理说明](反向代理.md) 手动配置。Agent 与终端仍使用 WSS。

若希望自主选择程序目录、系统用户和内部端口，使用下面的手动步骤。

## 二、宝塔或已有 Nginx 的手动安装

以下用 `probe.example.com` 举例，必须替换成自己的域名。使用独立项目目录 `/opt/YJ`，数据目录 `/var/lib/YJ`，内部端口 `19282`。部署第二套面板时不要共用这些目录、用户、服务名或内部端口。

### 1. 准备站点和证书

1. 安装 Nginx；此站点不需要 PHP 或 MySQL。
2. 在宝塔添加站点、绑定域名，PHP 版本选择“纯静态”。站点目录可使用空目录。
3. 申请并启用 SSL，确认域名通过 HTTPS 正常访问。保留宝塔的证书续期配置。
4. 内部端口只绑定 127.0.0.1，云安全组不开放 19282。
5. Debian/Ubuntu 安装维护工具：`apt-get update && apt-get install -y ca-certificates curl unzip python3 openssl`。RPM 系统使用相应 dnf 软件包。

### 2. 下载完整发行包

到 [v0.11.5 Release](https://github.com/YJVIst/YJmonitor/releases/tag/v0.11.5) 下载 `yuji-probe-rust-0.11.5.zip` 及同名 `.sha256` 文件。不要下载 GitHub 自动生成的 Source code 包：它没有预编译二进制。

```sh
install -d -m 700 /root/YJ-install
cd /root/YJ-install
curl -fLO --proto '=https' --proto-redir '=https' https://github.com/YJVIst/YJmonitor/releases/download/v0.11.5/yuji-probe-rust-0.11.5.zip
curl -fLO --proto '=https' --proto-redir '=https' https://github.com/YJVIst/YJmonitor/releases/download/v0.11.5/yuji-probe-rust-0.11.5.zip.sha256
sha256sum -c yuji-probe-rust-0.11.5.zip.sha256
unzip yuji-probe-rust-0.11.5.zip
test ! -e /opt/YJ
mv yuji-probe-rust-0.11.5 /opt/YJ
```

校验和用于核对下载内容；完整的签名校验由一键安装器和后台升级器执行。手动部署只从本仓库受信任的 Release 获取包和校验文件。

确认结构：

```text
/opt/YJ/
  bin/probe-linux-amd64
  bin/probe-linux-arm64
  ops/templates/nginx-rust.conf
  ops/templates/YJ.service
  source/                       # 源码，运行无需编译
  docs/
```

### 3. 自动准备服务账户与私有数据目录

以 root 执行源码包内的准备脚本：

~~~sh
python3 /opt/YJ/ops/prepare-service.py
~~~

脚本自动创建独立的服务账户、私有数据目录，并设置二进制执行权限。默认账户为 YJ，数据目录为 /var/lib/YJ/control，无需手动执行 useradd、mkdir 或 chown。重复运行只接受属于该实例且权限正确的目录；遇到符号链接、不匹配的已有账户或目录会停止，不接管其他程序的数据。

程序目录须由 root 管理，服务账户只写私有数据目录。使用一键安装时，这一步也会由安装器自动完成。

### 4. 创建 systemd 服务

查看 `uname -m`：x86_64 使用 amd64；aarch64 使用 arm64。

创建 `/etc/systemd/system/YJ.service`。下面以 amd64 为例，替换域名；ARM 服务器把程序名改为 `probe-linux-arm64`：

```ini
[Unit]
Description=Yuji Probe Nginx + Rust
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=YJ
Group=YJ
WorkingDirectory=/opt/YJ
ExecStart=/opt/YJ/bin/probe-linux-amd64 -state /var/lib/YJ/control -listen 127.0.0.1:19282 -origin https://probe.example.com -web
Restart=always
RestartSec=3
TimeoutStopSec=10
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/YJ
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictSUIDSGID=true
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
CapabilityBoundingSet=
LockPersonality=true
MemoryMax=512M
TasksMax=128

[Install]
WantedBy=multi-user.target
```

`MemoryMax` 是保护上限，不是预留或实际占用。根据规模与备份/主题处理峰值调整，资源比较见验收报告。

```sh
systemctl daemon-reload
systemctl enable --now YJ
systemctl status YJ --no-pager
```

首次启动会生成私有安装链接；还没有创建任何管理员。程序不会在日志中输出密码或安装密钥。

### 5. 配置本网站 Nginx 反向代理

请按 [宝塔 Nginx 配置位置与完整示例](宝塔Nginx配置说明.md) 操作。此前教程没有区分宝塔的两个编辑入口，现已按实际部署补齐。

默认采用现有生产站的两份文件布局：

| 放置位置 | 内容 |
| --- | --- |
| 网站 → 配置文件 → HTTPS `server` 块内 | 域名、SSL、ACME，以及下列站点参数和 `include`。 |
| 网站 → 伪静态 | 详细说明中“方案一第二步”的完整 `location` 代理规则；不要放入外层 `server` 块。 |

在主配置中修改或补充以下内容；已有同名参数和 `include` 行时直接沿用或修改，不重复追加。请把示例域名替换为自己的域名：

~~~nginx
autoindex off;
client_max_body_size 24m;
client_header_timeout 10s;
client_body_timeout 10s;
send_timeout 15s;
keepalive_timeout 30s;
# 不将安装链接的查询参数写入访问日志；保留站点原有 error_log。
access_log off;

include /www/server/panel/vhost/rewrite/probe.example.com.conf;
~~~

随后把 [完整代理规则](宝塔Nginx配置说明.md#第二步网站伪静态) 放入该站点“伪静态”。主配置中的 `include` 会将它加载进相同的 `server` 上下文，因此其中的 `proxy_pass` 是正常的反向代理。

也可以选择 [方案二：全部放进主配置](宝塔Nginx配置说明.md#方案二所有内容集中放在主配置)，使用 [完整片段](../ops/templates/nginx-baota-rust.server.inc)，并将该站点伪静态文件留空或仅写注释。两种方案不能叠加，否则会产生重复 `location`。

保留该站点证书与 ACME 配置，移除旧 PHP、冲突的 `location /` 以及直接从本地目录提供 JS/CSS/图片的旧规则。详细说明包含六项参数解释、端口对应关系、WebSocket 设置、备份、加载检查及常见错误处理。

编辑两份文件后执行：

~~~sh
/www/server/nginx/sbin/nginx -t
~~~

检查成功后执行：

~~~sh
/www/server/nginx/sbin/nginx -s reload
~~~

使用当前实际运行的 Nginx 路径；普通发行版通常直接使用 `nginx`。操作范围仅限本网站，其他网站的 Nginx 配置和 PHP 服务继续运行。

启用 SELinux 的系统使用发行包内 `ops/templates/yuji-probe.cil`，将内部监听端口标记为 `yuji_probe_port_t`，程序、状态与证书设置对应标签；保持 enforcing。自定义目录与端口时参照 `install.sh` 的 SELinux 段调整。

### 6. 使用网页安装向导

在服务器终端运行：

```sh
cat /var/lib/YJ/control/setup-link.txt
```

复制输出的完整 HTTPS 链接到自己的浏览器。该链接含安装所有权凭据，不发给他人。填写站点名称、管理员用户名与密码，提交后跳转登录页面。首次访问普通域名只显示安装尚未完成，不能抢先创建管理员。

安装完成后，链接文件会删除，安装入口锁定。管理员密码保存为 bcrypt 哈希。访问域名、登录、启用二步验证，再添加服务器。无需数据库配置。

### 7. 启用后台签名更新与回退

以 root 执行：

```sh
install -d -m 700 /etc/YJ
python3 /opt/YJ/ops/manage.py configure https://probe.example.com 19282
python3 /opt/YJ/ops/install-command.py
sudo YJ
```

版本管理只检查本仓库 `v*` 的正式发行版。管理员确认更新后，独立 root 更新服务下载并校验签名，保存上一版与私有状态，更新、重启并做健康检查；失败自动恢复，也可手动回退。不是发现新版后无人值守自动升级。

## 三、从现有 PHP 版迁移

建议先在独立测试站验证。迁移本网站时：

1. 在后台生成加密备份并下载到站点之外，同时保留旧程序与旧 Nginx 配置。
2. 暂停管理员操作，结束终端和传输。只停止探针自己的旧主控/专用 PHP 池，保持其他网站服务运行。
3. 把旧 `storage/control` **完整复制**到新私有状态目录，保留 `app.key`、认证配置、节点配置、加密密钥与 SSH 指纹。不可只复制 nodes.json，也不运行新安装向导覆盖原配置。
4. 将数据目录所有者改为新的服务用户，目录 700、文件 600。旧副本保留在只有 root 可读的位置。如旧状态中存在手动准备的 `control/releases` 离线 Agent 缓存，先核对 `stable.json` 的已签名版本。本版需要 Agent 0.2.3；旧版缓存应移至站点之外备份，让新主控从新仓库取得匹配的发布文件。此缓存不包含节点认证密钥，勿将整个 control 当作缓存清理。
5. 用新 Rust 服务以相同 `-origin` 启动，修改该域名的 Nginx 代理端口，检查后 reload。
6. 验证原管理员登录、所有节点上线、SSH/文件操作、通知和备份。主控重启会使旧登录过期，需重新登录。
7. 重新用 `--distribution rust` 注册这一实例的更新器。停用旧实例更新监听，避免两个更新器同时操作同一状态。
8. 验收后再清理本网站不再使用的 PHP 池。不要停掉其他网站正在使用的 PHP-FPM。

同域名且完整保留状态时，Agent 不需要重新录入节点密钥。若同时更换域名，使用面板的加密备份恢复/迁移功能按其提示重新配置 Agent，不只修改 Nginx 域名。

需要回到旧 PHP 版时，停止新服务，恢复迁移前的程序、状态副本和该站点 Nginx 配置，再启动旧的专用服务。不要让两套主控同时写同一个数据目录。

## 四、终端行为与故障排查

Agent 需更新到 0.2.2，安装/更新时会准备 tmux。SSH 终端、目录浏览/编辑、文件传输各用按需建立的独立 SSH 连接，仍通过经认证的 WSS Agent 通道传输。

异常断线后原 Shell 最多保留五分钟，只允许原管理登录恢复；主动断开、关闭终端标签、退出登录会结束该 Shell。主控重启、远端重启或远端主动退出不能恢复。命令不会自动重发。详见 [终端与文件操作](终端与文件操作.md)。

- 502：检查 systemd 服务、内部监听端口、目录所有权与日志。
- 421：检查 `-origin`、访问域名、HTTPS 端口和 Nginx Host 头一致。
- 安装提示私有链接：在服务器读取 setup-link.txt；已安装时不要删除 auth.json 强行重开安装。
- WebSocket 失败：检查对应 location 的 Upgrade/Connection 头、代理超时和 HTTPS 证书。
- 无法建立可恢复终端：先更新 Agent 并确认目标服务器已安装 tmux；检查 SSH 指纹与用户权限。
- 升级失败：查看版本管理提示和 `journalctl -u yuji-probe-rust-update-rust-main`，签名或校验失败时不要跳过校验。

本版不要求打开新的公网管理端口。Nginx 接受 443，Rust 主控只监听回环地址，Agent 主动连接主控 WSS。

## 五、从原仓库的 rust-v0.10.0 切换

已有 Rust 安装保留原域名、服务名、服务用户、端口和完整私有状态。下载本仓库发行包，仅替换程序，重新注册本仓库的更新服务。不要运行全新安装器覆盖原配置。具体操作和兼容边界见 [仓库迁移说明](仓库迁移-0.10.1.md)。
