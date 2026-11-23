#!/usr/bin/env bash
# One-time transition for installed Rust panels. Obtain this launcher from the official release.
set -eu
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 077
exec python3 -I - "$@" <<'PY'
import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import stat
import subprocess
import sys
import tempfile
import time
import zipfile

VERSION = "0.11.6"
OLD_KEY = "o8+DdHbo82V7fxJEIiEhe5AK/frR91Fz5vjf/pDAnts="
NEW_KEY = "5knizERV08A6pN/RMSh6ssDTK22NWm0U3+uCe8/4Azk="
REPO = "YJVIst/YJmonitor"
sys.dont_write_bytecode = True
CONFIG_DIRS = (Path("/etc/YJ/update"), Path("/etc/yuji-probe-rust-updaters"))

def trusted(path, directory=False):
    path = Path(path)
    if not path.is_absolute() or path.resolve(strict=True) != path:
        raise ValueError(f"路径包含链接或不是绝对路径：{path}")
    for item in (*reversed(path.parents), path):
        info = item.lstat()
        kind = stat.S_ISDIR if item != path or directory else stat.S_ISREG
        if not kind(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
            raise ValueError(f"需要 root 所有且仅 root 可写：{item}")
    return path

def module(path):
    spec = importlib.util.spec_from_file_location("yj_signed_updater", path)
    updater = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(updater)
    return updater

def installed_version(root):
    arch = {"x86_64": "amd64", "aarch64": "arm64"}[platform.machine()]
    binary = trusted(root / "bin" / ("probe-linux-" + arch))
    words = subprocess.check_output([binary, "--version"], text=True, timeout=10).split()
    if len(words) < 2 or words[0] != "vistart-probe-controller" or not re.fullmatch(r"[0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,5}", words[1]):
        raise ValueError("无法识别已安装的主控版本")
    return words[1]

def main():
    p = argparse.ArgumentParser(description="保留实例状态，迁移到 YJmonitor 新发布密钥并升级至 " + VERSION)
    p.add_argument("--instance", help="多个实例时指定已登记的实例名")
    p.add_argument("--release-dir", type=Path, help="从本地读取已下载的正式发行包和签名清单")
    p.add_argument("--check", action="store_true", help="仅检查当前安装并验签发行包")
    args = p.parse_args()
    if os.geteuid() != 0:
        raise ValueError("请使用 root 运行")
    configs = [f for directory in CONFIG_DIRS if directory.exists()
               for f in trusted(directory, True).glob("*.json") if not args.instance or f.stem == args.instance]
    if len(configs) != 1:
        raise ValueError("需要唯一的已登记 Rust 实例，请检查更新服务或用 --instance 指定：" + ", ".join(f.stem for f in configs))
    path = trusted(configs[0])
    cfg = json.loads(path.read_bytes())
    if cfg.get("distribution") != "rust" or cfg["name"] != path.stem or not re.fullmatch("[a-z0-9-]{1,40}", path.stem):
        raise ValueError("不支持的实例配置")
    root = trusted(cfg["root"], True)
    old = module(trusted(root / "ops/update-panel.py"))
    if old.PUBLIC not in (OLD_KEY, NEW_KEY):
        raise ValueError("当前实例使用其他发布密钥")
    # Explicitly running this fixed launcher authorizes only this key transition.
    # The installed updater's download allowlist and signature verification stay active.
    old.REPOSITORY = REPO
    installed_key = old.PUBLIC
    old.PUBLIC = NEW_KEY
    if hasattr(old, "select_layout"):
        old.select_layout(root)
    work = old.BASE / cfg["name"]
    old.private(work)
    with (work / "lock").open("a") as lock, tempfile.TemporaryDirectory(prefix="yj-key-migration-") as scratch:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        current = installed_version(root)
        if tuple(map(int,current.split("."))) > tuple(map(int,VERSION.split("."))):
            raise ValueError("已安装更高版本，请使用该版本的管理入口")
        if current == VERSION and installed_key != NEW_KEY:
            raise ValueError("已有同版本自定义构建，请核对安装来源")
        cache = Path(scratch)
        signed = old.download(VERSION, "panel-stable.json", 16384, args.release_dir)
        entry = old.verify(signed, VERSION, cache)
        archive = old.download(VERSION, entry["name"], 64*1024*1024, args.release_dir)
        if len(archive) != entry["size"] or hashlib.sha256(archive).hexdigest() != entry["sha256"]:
            raise ValueError("安装包与新签名清单不一致")
        (cache / "panel-stable.json").write_bytes(signed)
        bundle = cache / entry["name"]
        bundle.write_bytes(archive)
        del archive
        # The new signed updater handles snapshots across filesystems and rollback.
        with zipfile.ZipFile(bundle) as z:
            helper = cache / "update-panel.py"
            helper.write_bytes(z.read(f"yuji-probe-rust-{VERSION}/ops/update-panel.py"))
        new = module(helper)
        if new.PUBLIC != NEW_KEY or new.REPOSITORY != REPO:
            raise ValueError("发行包内的更新器与本次迁移不匹配")
        if args.check:
            print(f"{cfg['name']}：{current} → {VERSION}，新版签名和安装包校验通过")
            return
        if current != VERSION:
            new.apply(cfg, {"action":"update", "version":VERSION, "at":int(time.time())}, cache)
        # Retrying after a registration failure completes this step without replacing the snapshot.
        subprocess.run([sys.executable, "-I", str(trusted(root / "ops/update-panel.py")), "--configure",
                        "--name", cfg["name"], "--root", str(root), "--state", cfg["state"],
                        "--service", cfg["service"], "--origin", cfg["origin"], "--listen", cfg["listen"],
                        "--distribution", "rust"], check=True)
        new.check(cfg, VERSION)
        print(f"{cfg['name']}：主控 {VERSION} 健康检查通过。请在面板的版本管理中选择在线旧节点并批量升级。")

try:
    main()
except (Exception, KeyboardInterrupt) as error:
    sys.exit("迁移未完成：" + str(error))
PY
