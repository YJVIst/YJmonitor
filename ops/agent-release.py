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
