#!/usr/bin/env bash
set +x
set -Eeuo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 077
readonly YUJI_VERSION=0.11.5
readonly YUJI_RELEASE_BASE=https://github.com/YJVIst/YJmonitor/releases/download/v0.11.5
readonly YUJI_PUBLIC_KEY=o8+DdHbo82V7fxJEIiEhe5AK/frR91Fz5vjf/pDAnts=
yuji_work='' yuji_name='' yuji_port=19281 yuji_cache='' yuji_check=0 yuji_repair=0
prepare_agent_release(){
 timeout 240 python3 - "$@" <<'PYAGENTRELEASE'
#!/usr/bin/env python3
"""Fetch the signed Agent release from GitHub for existing Rust controllers."""
import argparse
import base64
import hashlib
import json
import os
import pathlib
import re
import ssl
import stat
import subprocess
import tempfile
import urllib.parse
import urllib.request

REPOSITORY = "YJVIst/YJmonitor"
RELEASE_VERSION = "0.11.3"
AGENT_VERSION = "0.2.3"
PUBLIC = "o8+DdHbo82V7fxJEIiEhe5AK/frR91Fz5vjf/pDAnts="
PREFIX = f"/{REPOSITORY}/releases/download/v{RELEASE_VERSION}/"
BASE = "https://github.com" + PREFIX


def allowed(url):
    try:
        u = urllib.parse.urlsplit(url)
        if (u.scheme != "https" or u.port not in (None, 443)
                or u.username or u.password or u.fragment):
            return False
    except ValueError:
        return False
    if u.hostname == "github.com":
        name = u.path.removeprefix(PREFIX)
        return (u.path.startswith(PREFIX) and not u.query
                and re.fullmatch(r"[A-Za-z0-9_.-]+", name) is not None
                and name not in (".", ".."))
    return (
        u.hostname == "release-assets.githubusercontent.com"
        and u.path.startswith("/github-production-release-asset/")
    ) or (
        u.hostname == "objects.githubusercontent.com"
        and u.path.startswith("/github-production-release-asset-2e65be/")
    )


class Redirect(urllib.request.HTTPRedirectHandler):
    max_redirections = 4

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not allowed(newurl):
            raise ValueError("Agent 下载被引导至非指定 GitHub 发布地址")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def download(name, limit, offline=None):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", name) or name in (".", ".."):
        raise ValueError("Agent 文件名无效")
    if offline:
        source = pathlib.Path(offline) / name
        info = source.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0
                or info.st_mode & 0o022 or info.st_size > limit):
            raise ValueError("离线 Agent 文件权限或大小不正确")
        raw = source.read_bytes()
    else:
        client = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), Redirect(),
            urllib.request.HTTPSHandler(context=ssl.create_default_context()),
        )
        request = urllib.request.Request(
            BASE + name, headers={"User-Agent": "YJmonitor-Agent-Release/1"},
        )
        with client.open(request, timeout=30) as response:
            if response.status != 200:
                raise ValueError("Agent 发布下载失败")
            raw = response.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("Agent 发布文件超出大小限制")
    return raw


def verify_manifest(raw, work):
    envelope = json.loads(raw)
    payload = base64.b64decode(envelope["payload"], validate=True)
    signature = base64.b64decode(envelope["signature"], validate=True)
    if len(payload) > 8192 or len(signature) != 64:
        raise ValueError("Agent 签名格式不正确")
    (work / "public.der").write_bytes(
        bytes.fromhex("302a300506032b6570032100") + base64.b64decode(PUBLIC)
    )
    (work / "payload").write_bytes(payload)
    (work / "signature").write_bytes(signature)
    subprocess.run(
        ["openssl", "pkeyutl", "-verify", "-pubin", "-keyform", "DER",
         "-inkey", str(work / "public.der"), "-rawin",
         "-in", str(work / "payload"), "-sigfile", str(work / "signature")],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        timeout=15,
    )
    manifest = json.loads(payload)
    if manifest["version"] != AGENT_VERSION:
        raise ValueError("Agent 版本与现有主控不兼容")
    for arch in ("amd64", "arm64"):
        entry = manifest["files"][arch]
        if (entry["name"] != f"vistart-probe-agent-{AGENT_VERSION}-linux-{arch}"
                or type(entry["size"]) is not int
                or not 1024 <= entry["size"] <= 32 * 1024 * 1024
                or not isinstance(entry["sha256"], str)
                or not re.fullmatch(r"[a-f0-9]{64}", entry["sha256"])):
            raise ValueError("Agent 签名清单内容不正确")
    return manifest


def verify_binary(raw, entry, arch):
    if (len(raw) != entry["size"]
            or hashlib.sha256(raw).hexdigest() != entry["sha256"]):
        raise ValueError("Agent 哈希或长度校验失败：" + arch)
    machine = 62 if arch == "amd64" else 183
    if (raw[:6] != b"\x7fELF\x02\x01"
            or int.from_bytes(raw[18:20], "little") != machine):
        raise ValueError("Agent 二进制架构不正确：" + arch)


def state_directory(explicit):
    if explicit:
        state = pathlib.Path(explicit)
    else:
        states = set()
        for folder in ("/etc/YJ/update", "/etc/yuji-probe-rust-updaters"):
            for config in pathlib.Path(folder).glob("*.json"):
                info = config.lstat()
                if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0
                        or info.st_mode & 0o022):
                    raise ValueError("主控实例配置权限不正确")
                item = json.loads(config.read_text())
                if item.get("distribution") == "rust":
                    states.add(item["state"])
        if not states and pathlib.Path("/var/lib/YJ/control/nodes.json").is_file():
            states.add("/var/lib/YJ/control")
        if len(states) != 1:
            raise ValueError("请用 --state 指定 Rust 主控的数据目录")
        state = pathlib.Path(states.pop())
    if not state.is_absolute() or state.resolve() != state:
        raise ValueError("主控数据目录必须为不含符号链接的绝对路径")
    info = state.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_mode & 0o022:
        raise ValueError("主控数据目录权限不正确")
    return state, info.st_uid, info.st_gid


def publish(state, uid, gid, files):
    target = state / "releases"
    if target.exists() or target.is_symlink():
        info = target.lstat()
        if (not stat.S_ISDIR(info.st_mode)
                or info.st_uid not in (0, uid) or info.st_mode & 0o022):
            raise ValueError("Agent 缓存目录权限或类型不正确")
    else:
        target.mkdir(mode=0o700)
    os.chown(target, uid, gid)
    target.chmod(0o700)
    # All files have passed signature/digest checks. Publish the manifest last.
    for name, raw in files.items():
        fd, temporary = tempfile.mkstemp(prefix=".agent-release-", dir=target)
        try:
            os.fchmod(fd, 0o600)
            os.fchown(fd, uid, gid)
            with os.fdopen(fd, "wb") as output:
                output.write(raw)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, target / name)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    directory = os.open(target, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", help="Rust 主控数据目录；默认自动识别")
    parser.add_argument("--release-dir", type=pathlib.Path, help="已下载的签名发布目录")
    parser.add_argument("--verify-only", action="store_true", help="只下载并校验，不写入主控")
    args = parser.parse_args()
    if not args.verify_only and os.geteuid() != 0:
        raise ValueError("请使用 root 运行")
    destination = None if args.verify_only else state_directory(args.state)
    with tempfile.TemporaryDirectory(prefix="YJ-agent-release-") as directory:
        manifest_raw = download("stable.json", 16384, args.release_dir)
        manifest = verify_manifest(manifest_raw, pathlib.Path(directory))
        files = {}
        for arch in ("amd64", "arm64"):
            entry = manifest["files"][arch]
            raw = download(entry["name"], entry["size"], args.release_dir)
            verify_binary(raw, entry, arch)
            files[entry["name"]] = raw
        files["stable.json"] = manifest_raw
        if destination:
            target = publish(*destination, files)
            print(f"Agent {AGENT_VERSION} 已从 GitHub 验签并准备完成：{target}")
        else:
            print(f"GitHub Agent {AGENT_VERSION}：amd64/arm64 签名、SHA-256、长度和架构校验通过")


if __name__ == "__main__":
    main()
PYAGENTRELEASE
}
die(){ printf '\n  %s\n\n' "$*" >&2; exit 1; }
cleanup(){ local rc=$?; [[ -z "$yuji_work" ]] || rm -rf -- "$yuji_work"; exit "$rc"; }
trap cleanup EXIT
while (( $# )); do
 case "$1" in
  --site-name|--port|--release-dir)
   (( $# >= 2 )) || die "$1 缺少参数"
   case "$1" in --site-name) yuji_name=$2;; --port) yuji_port=$2;; --release-dir) yuji_cache=$2;; esac
   shift 2;;
  --check) yuji_check=1;shift;;
  --repair-agent-source) yuji_repair=1;shift;;
  -h|--help)
   printf '%s\n' '羽迹探针 · Rust' '' '用法：bash install.sh [--site-name 站点名称] [--port 19281]' '      bash install.sh --check' '      bash install.sh --repair-agent-source' '' '交互安装只需填写站点名称。自动生成管理员密码，显示 IP:端口。' '支持 Debian 12/13、Ubuntu 22.04/24.04/26.04、Rocky/AlmaLinux 9/10、' 'CentOS Stream 9/10、Fedora 42/43/44，amd64/arm64，systemd。' '基础安装不改动 Nginx；安装后可在菜单 13 自动配置 Nginx + HTTPS。'
   exit 0;;
  *) die "未知参数：$1";;
 esac
done
(( EUID == 0 )) || die '请使用 root 运行。'
[[ -f /etc/os-release ]] || die '无法识别操作系统。'
# shellcheck disable=SC1091
. /etc/os-release
case "${ID:-}:${VERSION_ID:-}" in
 debian:12|debian:13|ubuntu:22.04|ubuntu:24.04|ubuntu:26.04) yuji_platform=apt;;
 rocky:9|rocky:9.*|rocky:10|rocky:10.*|almalinux:9|almalinux:9.*|almalinux:10|almalinux:10.*|fedora:42|fedora:43|fedora:44) yuji_platform=dnf;;
 centos:9|centos:10) [[ ${NAME:-} == *Stream* ]] || die '仅支持 CentOS Stream 9/10。';yuji_platform=dnf;;
 *) die '此系统未在安装脚本的支持列表中，请查看 --help。';;
esac
[[ -d /run/systemd/system ]] || die '需要使用 systemd 的 Linux 系统。'
case "$(uname -m)" in x86_64) yuji_arch=amd64;; aarch64|arm64) yuji_arch=arm64;; *) die '仅支持 amd64、arm64。';; esac
if ! [[ "$yuji_port" =~ ^[0-9]{4,5}$ ]] || (( 10#$yuji_port < 1024 || 10#$yuji_port > 65535 )); then die '端口范围为 1024–65535。'; fi
if (( yuji_check )); then printf '支持安装：%s %s · %s\n' "$ID" "$VERSION_ID" "$yuji_arch";exit 0;fi
command -v flock >/dev/null || die '请先安装 util-linux。'
exec 9>/run/YJ-install.lock
flock -n 9 || die '另一项安装正在进行。'
if (( yuji_repair )); then
 prepare_agent_release
 exit 0
fi
if [[ -f /opt/YJ/manage.sh && -f /etc/YJ/instance.json ]]; then
 python3 /opt/YJ/ops/install-command.py
 exec bash /opt/YJ/manage.sh
fi
# Never replace or silently duplicate an existing standard installation.
if [[ -f /opt/yuji-probe/manage.sh && -f /etc/yuji-probe/instance.json ]]; then
 [[ ! -f /opt/yuji-probe/ops/install-command.py ]] || python3 /opt/yuji-probe/ops/install-command.py --root /opt/yuji-probe
 exec bash /opt/yuji-probe/manage.sh
fi
yuji_resume=0
yuji_partial_clear=1
for yuji_path in /var/lib/YJ-update /usr/local/lib/YJ /etc/systemd/system/YJ-update-main.path /etc/systemd/system/YJ-update-main.service /opt/yuji-probe /var/lib/yuji-probe /etc/yuji-probe /etc/systemd/system/yuji-probe.service /usr/local/bin/yuji-probe; do
 [[ ! -e "$yuji_path" && ! -L "$yuji_path" ]] || yuji_partial_clear=0
done
if (( yuji_partial_clear )) && [[ -f /etc/YJ/initial-admin.json && ! -L /etc/YJ/initial-admin.json && ! -s /etc/YJ/initial-admin.json ]]; then
 # A prior run can stop while writing this file after preparing the service
 # account and private data directory. Resume only that exact, incomplete state.
 yuji_passwd=$(getent passwd YJ || true)
 if [[ -n "$yuji_passwd" && -d /var/lib/YJ && ! -L /var/lib/YJ && -d /var/lib/YJ/control && ! -L /var/lib/YJ/control && -L /opt/YJ/storage && $(readlink -- /opt/YJ/storage) == /var/lib/YJ && -f /opt/YJ/manage.sh && -f /opt/YJ/bin/probe-linux-amd64 && -f /opt/YJ/bin/probe-linux-arm64 && -d /etc/YJ && ! -L /etc/YJ && ! -e /etc/YJ/instance.json && ! -e /etc/systemd/system/YJ.service && ! -e /usr/local/bin/YJ && $(stat -c '%u:%g:%a' /etc/YJ) == 0:0:700 && $(stat -c '%u:%g:%a' /etc/YJ/initial-admin.json) == 0:0:600 ]]; then
  IFS=: read -r _ _ yuji_uid yuji_gid _ yuji_home yuji_shell <<< "$yuji_passwd"
  yuji_group=$(getent group YJ || true)
  IFS=: read -r _ _ yuji_group_gid _ <<< "$yuji_group"
  if [[ "$yuji_uid" =~ ^[0-9]+$ && "$yuji_uid" != 0 && "$yuji_gid" == "$yuji_group_gid" && "$yuji_home" == /var/lib/YJ && "$yuji_shell" == /usr/sbin/nologin && $(stat -c '%u:%g:%a' /var/lib/YJ) == "$yuji_uid:$yuji_gid:700" && $(stat -c '%u:%g:%a' /var/lib/YJ/control) == "$yuji_uid:$yuji_gid:700" ]]; then
   yuji_resume=1
  fi
 fi
fi
if (( yuji_resume )); then
 printf '\n  检测到未完成的安装，将保留 /var/lib/YJ 中的数据并继续。\n'
else
 for yuji_path in /var/lib/YJ-update /usr/local/lib/YJ /etc/systemd/system/YJ-update-main.path /etc/systemd/system/YJ-update-main.service /opt/yuji-probe /var/lib/yuji-probe /etc/yuji-probe /etc/systemd/system/yuji-probe.service /opt/YJ /var/lib/YJ /etc/YJ /etc/systemd/system/YJ.service /usr/local/bin/YJ /usr/local/bin/yuji-probe; do
  [[ ! -e "$yuji_path" && ! -L "$yuji_path" ]] || die "已有 $yuji_path，安装已停止。现有站点请按升级文档操作。"
 done
 getent passwd YJ >/dev/null && die 'YJ 系统账户已存在，请先核对现有部署。'
 getent group YJ >/dev/null && die 'YJ 系统组已存在，请先核对现有部署。'
fi
printf '\n  羽迹探针  /  安装\n  ──────────────────────────────\n  版本  %s    端口  %s\n\n' "$YUJI_VERSION" "$yuji_port"
if [[ -z "$yuji_name" ]]; then
 exec 3<>/dev/tty || die '需要交互终端，或使用 --site-name。'
 read -r -u 3 -p '  站点名称：' yuji_name
fi
[[ -n "$yuji_name" && ${#yuji_name} -le 60 && ! "$yuji_name" =~ [[:cntrl:]] ]] || die '站点名称需要 1–60 个字符。'
printf '\n  正在准备运行环境…\n'
if [[ "$yuji_platform" == apt ]]; then
 export DEBIAN_FRONTEND=noninteractive
 apt-get update -qq
 apt-get install -y --no-install-recommends ca-certificates curl openssl python3 sudo iproute2 util-linux iputils-tracepath
else
 yuji_curl_package=();command -v curl >/dev/null || yuji_curl_package=(curl-minimal)
 dnf install -y ca-certificates "${yuji_curl_package[@]}" openssl python3 sudo iproute util-linux iputils
fi
[[ -z "$(ss -H -ltn "( sport = :$yuji_port )")" ]] || die "端口 $yuji_port 已被使用，请用 --port 指定其他端口。"
yuji_work=$(mktemp -d /var/tmp/YJ-install.XXXXXXXX)
printf '\n  正在下载并验证发行包…\n'
timeout 240 python3 - "$YUJI_RELEASE_BASE" "$YUJI_PUBLIC_KEY" "$yuji_work" "$YUJI_VERSION" "$yuji_cache" <<'PYDOWNLOAD'
import base64, hashlib, json, pathlib, ssl, stat, subprocess, sys, urllib.parse, urllib.request, zipfile
base, key, work, version, cache = sys.argv[1:]
root = pathlib.Path(work)
prefix = urllib.parse.urlsplit(base).path + "/"
def allowed(url):
    u = urllib.parse.urlsplit(url)
    if u.scheme != "https" or u.port not in (None, 443) or u.username or u.password or u.fragment:
        return False
    return ((u.hostname == "github.com" and u.path.startswith(prefix) and not u.query)
        or (u.hostname == "release-assets.githubusercontent.com" and u.path.startswith("/github-production-release-asset/"))
        or (u.hostname == "objects.githubusercontent.com" and u.path.startswith("/github-production-release-asset-2e65be/")))
class Redirect(urllib.request.HTTPRedirectHandler):
    max_redirections = 4
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not allowed(newurl): raise RuntimeError("下载被引导至非 GitHub HTTPS 发布地址")
        return super().redirect_request(req, fp, code, msg, headers, newurl)
client = urllib.request.build_opener(urllib.request.ProxyHandler({}), Redirect(), urllib.request.HTTPSHandler(context=ssl.create_default_context()))
def download(name, limit):
    if cache:
        p=pathlib.Path(cache)/name
        info=p.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid!=0 or info.st_mode&0o022 or info.st_size>limit: raise RuntimeError("离线发布文件权限或大小不正确")
        return p.read_bytes()
    url = base + "/" + name
    if not allowed(url): raise RuntimeError("无效的 GitHub 发布地址")
    with client.open(urllib.request.Request(url, headers={"User-Agent":"Yuji-Probe-Installer/"+version}), timeout=25) as response:
        if response.status != 200: raise RuntimeError("下载失败")
        raw = response.read(limit + 1)
    if len(raw) > limit: raise RuntimeError("发布文件超出限制")
    return raw
envelope = json.loads(download("panel-stable.json", 65536))
payload = base64.b64decode(envelope["payload"], validate=True)
signature = base64.b64decode(envelope["signature"], validate=True)
public = base64.b64decode(key, validate=True)
if len(public) != 32 or len(signature) != 64: raise RuntimeError("无效签名")
(root/"payload").write_bytes(payload)
(root/"signature").write_bytes(signature)
(root/"public.der").write_bytes(bytes.fromhex("302a300506032b6570032100")+public)
subprocess.run(["openssl","pkeyutl","-verify","-pubin","-keyform","DER","-inkey",str(root/"public.der"),"-rawin","-in",str(root/"payload"),"-sigfile",str(root/"signature")],check=True,stdout=subprocess.DEVNULL)
manifest=json.loads(payload)
entry=manifest["files"]["panel"]
name="yuji-probe-rust-"+version+".zip"
if manifest["version"] != version or entry["name"] != name or not 1024 <= entry["size"] <= 128*1024*1024:
    raise RuntimeError("发行版本或文件名称不匹配")
data=download(name, entry["size"])
if len(data) != entry["size"] or hashlib.sha256(data).hexdigest() != entry["sha256"]:
    raise RuntimeError("发行包哈希或大小不匹配")
archive=root/name
archive.write_bytes(data)
prefix="yuji-probe-rust-"+version+"/"
with zipfile.ZipFile(archive) as z:
    if len(z.infolist()) > 10000 or sum(i.file_size for i in z.infolist()) > 256*1024*1024:
        raise RuntimeError("发行包解压大小超出限制")
    seen=set()
    for i in z.infolist():
        name=i.filename
        path=pathlib.PurePosixPath(name)
        kind=stat.S_IFMT(i.external_attr >> 16)
        if not name.startswith(prefix) or "\\" in name or path.is_absolute() or ".." in path.parts or name in seen or kind not in (0,stat.S_IFREG,stat.S_IFDIR):
            raise RuntimeError("发行包包含无效路径或特殊文件")
        seen.add(name)
    z.extractall(root/"unpacked")
package=root/"unpacked"/prefix.rstrip("/")
for required in ["bin/probe-linux-amd64","bin/probe-linux-arm64","ops/templates/nginx-rust.conf","ops/templates/YJ.service"]:
    if not (package/required).is_file(): raise RuntimeError("发行包缺少必要文件")
print("发行包 Ed25519 签名、SHA-256、大小与解压路径校验通过。")
PYDOWNLOAD
install -d -m 0700 "$yuji_work/agent-state"
yuji_agent_args=(--state "$yuji_work/agent-state")
[[ -z "$yuji_cache" ]] || yuji_agent_args+=(--release-dir "$yuji_cache")
prepare_agent_release "${yuji_agent_args[@]}"
yuji_ip=$(python3 - <<'PYIP'
import ipaddress,socket,urllib.request
try:
 with urllib.request.urlopen('https://api.ipify.org',timeout=6) as r: ip=r.read(64).decode().strip()
 assert ipaddress.ip_address(ip).is_global
 print(ip)
except Exception:
 s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.connect(('1.1.1.1',53));print(s.getsockname()[0]);s.close()
PYIP
)
yuji_origin="http://$yuji_ip:$yuji_port"
if (( !yuji_resume )); then
 install -d -m 0755 /opt/YJ
 cp -a "$yuji_work/unpacked/yuji-probe-rust-$YUJI_VERSION/." /opt/YJ/
 find /opt/YJ -type d -exec chmod 0755 {} +
 find /opt/YJ -type f -exec chmod 0644 {} +
 chmod 0755 /opt/YJ/bin/probe-linux-* /opt/YJ/manage.sh
 rm -f /opt/YJ/storage/.gitkeep
 rmdir /opt/YJ/storage
fi
python3 /opt/YJ/ops/prepare-service.py --root /opt/YJ --data /var/lib/YJ --user YJ
prepare_agent_release --state /var/lib/YJ/control --release-dir "$yuji_work/agent-state/releases"
if [[ ! -L /opt/YJ/storage ]]; then ln -s /var/lib/YJ /opt/YJ/storage; fi
install -d -m 0700 /etc/YJ
python3 - "$yuji_name" "$yuji_origin" <<'PYINIT'
import json,os,pathlib,secrets,sys,tempfile
def utf8_text(value):
    return os.fsencode(value).decode("utf-8", "replace")
name,origin=(utf8_text(value) for value in sys.argv[1:])
password=secrets.token_urlsafe(24)
path=pathlib.Path('/etc/YJ/initial-admin.json')
fd,tmp=tempfile.mkstemp(prefix='.initial-admin.',dir=path.parent)
try:
    with os.fdopen(fd,'w',encoding='utf-8') as output:
        output.write(json.dumps({'name':name,'username':'admin','password':password,'url':origin},ensure_ascii=False)+'\n')
        output.flush()
        os.fsync(output.fileno())
    os.chmod(tmp,0o600)
    os.replace(tmp,path)
finally:
    try: os.unlink(tmp)
    except FileNotFoundError: pass
PYINIT
python3 -c 'import json;d=json.load(open("/etc/YJ/initial-admin.json"));d.pop("url");print(json.dumps(d))' | runuser -u YJ -- /opt/YJ/bin/probe-linux-"$yuji_arch" -web -state /var/lib/YJ/control -origin "$yuji_origin" --install
python3 /opt/YJ/ops/manage.py configure "$yuji_origin" "$yuji_port"
python3 /opt/YJ/ops/install-command.py
printf '\n  安装完成\n  ──────────────────────────────\n'
python3 - <<'PYSHOW'
import json
d=json.load(open('/etc/YJ/initial-admin.json'))
print('  访问地址  '+d['url']+'\n  管理员    '+d['username']+'\n  初始密码  '+d['password'])
print('\n  管理菜单  sudo YJ\n  反向代理  https://github.com/YJVIst/YJmonitor/blob/main/docs/反向代理.md')
print('\n  自动 HTTPS：运行 sudo YJ，选择 13。域名需已解析到本机。')
print('\n  初始凭据保存在 /etc/YJ/initial-admin.json（仅 root 可读）。')
PYSHOW
