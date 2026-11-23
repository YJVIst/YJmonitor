#!/usr/bin/env bash
set +x
set -Eeuo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 077
exec python3 - "$@" <<'PY'
import argparse,fcntl,importlib.util,json,os,pathlib,re,stat,subprocess,sys,time
VERSION="0.11.5"
REPOSITORY="YJVIst/YJmonitor"
PUBLIC="o8+DdHbo82V7fxJEIiEhe5AK/frR91Fz5vjf/pDAnts="
def trusted(path,directory=False):
    path=pathlib.Path(path)
    for item in reversed(path.parents):
        s=item.lstat()
        if not stat.S_ISDIR(s.st_mode) or s.st_uid!=0 or s.st_mode&0o022:raise ValueError("不受信任的程序目录："+str(item))
    s=path.lstat()
    if not (stat.S_ISDIR(s.st_mode) if directory else stat.S_ISREG(s.st_mode)) or s.st_uid!=0 or s.st_mode&0o022:
        raise ValueError("文件所有者或权限不正确："+str(path))
    return path
def main():
    if os.geteuid()!=0:raise ValueError("请使用 root 运行")
    p=argparse.ArgumentParser(description="将已安装 Rust 主控切换到 YJmonitor 的签名发布通道，保留状态与节点。")
    p.add_argument("--instance");p.add_argument("--release-dir",type=pathlib.Path)
    args=p.parse_args()
    folders=[trusted(p,True) for p in ["/etc/YJ/update","/etc/yuji-probe-rust-updaters"] if pathlib.Path(p).exists()]
    names=sorted(p for folder in folders for p in folder.glob("*.json"))
    if args.instance:
        if not re.fullmatch("[a-z0-9-]{1,40}",args.instance):raise ValueError("实例名称无效")
        names=[path for path in names if path.stem==args.instance]
    if len(names)!=1:raise ValueError("请用 --instance 指定实例："+", ".join(x.stem for x in names))
    cfg=json.loads(trusted(names[0]).read_text())
    if cfg.get("distribution")!="rust" or cfg.get("name")!=names[0].stem:raise ValueError("不是受支持的 Rust 实例")
    root=trusted(cfg["root"],True);source=trusted(root/"ops/update-panel.py")
    spec=importlib.util.spec_from_file_location("trusted_updater",source)
    m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
    if m.PUBLIC!=PUBLIC:raise ValueError("已有安装的签名公钥不匹配")
    # Only the repository identity changes. Existing signature/archive checks stay active.
    m.REPOSITORY=REPOSITORY
    work=m.BASE/cfg["name"];m.private(work)
    with (work/"lock").open("a") as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        arch="arm64" if os.uname().machine=="aarch64" else "amd64"
        current=subprocess.check_output([str(trusted(root/"bin"/("probe-linux-"+arch))),"--version"],text=True,timeout=10).split()[1]
        if not re.fullmatch(r"[0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,5}",current):raise ValueError("无法识别当前版本")
        if tuple(map(int,current.split(".")))<tuple(map(int,VERSION.split("."))):
            current=m.apply(cfg,{"action":"update","version":VERSION,"at":int(time.time())},args.release_dir)
        new=trusted(root/"ops/update-panel.py")
        subprocess.run(["python3",str(new),"--configure","--name",cfg["name"],"--root",str(root),"--state",cfg["state"],"--service",cfg["service"],"--origin",cfg["origin"],"--listen",cfg["listen"],"--distribution","rust"],check=True)
        if root in (pathlib.Path("/opt/YJ"),pathlib.Path("/opt/yuji-probe")):
            subprocess.run(["python3",str(trusted(root/"ops/install-command.py"))],check=True)
        m.atomic(pathlib.Path(cfg["state"])/"update-result.json",{"state":"done","message":"已切换到 YJmonitor，当前版本 "+current,"at":int(time.time())},cfg["uid"],cfg["gid"])
    print("迁移完成："+REPOSITORY+" · "+current+" · "+cfg["origin"])
    if root in (pathlib.Path("/opt/YJ"),pathlib.Path("/opt/yuji-probe")):print("管理菜单：sudo YJ（旧命令继续可用）")
    else:print("手动部署的服务、目录和监听地址已保留；后台继续使用签名更新。")
try:main()
except (Exception,KeyboardInterrupt) as e:print("迁移未完成："+str(e),file=sys.stderr);sys.exit(1)
PY
