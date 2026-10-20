#!/usr/bin/env python3
"""Root-owned, fixed-repository signed updater. Browser input cannot select paths or commands."""
import http.client,socket
import argparse,base64,fcntl,hashlib,json,os,pathlib,pwd,re,shutil,ssl,stat,subprocess,tempfile,time,urllib.request,urllib.parse,urllib.error,zipfile
REPOSITORY="YJVIst/YJmonitor"
PUBLIC="o8+DdHbo82V7fxJEIiEhe5AK/frR91Fz5vjf/pDAnts="
BASE=pathlib.Path("/var/lib/yuji-probe-rust-updater")
LIB=pathlib.Path("/usr/local/lib/yuji-probe-rust-updater")
CONF=pathlib.Path("/etc/yuji-probe-rust-updaters")
def select_layout(root):
    global BASE,LIB,CONF
    if pathlib.Path(root)==pathlib.Path("/opt/YJ"):
        BASE=pathlib.Path("/var/lib/YJ-update");LIB=pathlib.Path("/usr/local/lib/YJ");CONF=pathlib.Path("/etc/YJ/update")
    else:
        BASE=pathlib.Path("/var/lib/yuji-probe-rust-updater");LIB=pathlib.Path("/usr/local/lib/yuji-probe-rust-updater");CONF=pathlib.Path("/etc/yuji-probe-rust-updaters")
if pathlib.Path(__file__).resolve().parent==pathlib.Path("/usr/local/lib/YJ") or pathlib.Path(__file__).resolve().parents[1]==pathlib.Path("/opt/YJ"):
    select_layout("/opt/YJ")
VERSION=re.compile(r"[0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,5}\Z")
def private(path):path.mkdir(parents=True,exist_ok=True,mode=0o700);path.chmod(0o700)
def atomic(path,data,uid=0,gid=0,mode=0o600):
    data=data if isinstance(data,bytes) else json.dumps(data,ensure_ascii=False).encode()
    fd,name=tempfile.mkstemp(prefix=".yuji-",dir=path.parent)
    try:
        os.fchmod(fd,mode);os.fchown(fd,uid,gid)
        with os.fdopen(fd,"wb") as f:f.write(data);f.flush();os.fsync(f.fileno())
        os.replace(name,path)
        d=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY)
        try:os.fsync(d)
        finally:os.close(d)
    finally:
        if os.path.exists(name):os.unlink(name)
def trusted_file(path,limit):
    fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    try:
        st=os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_uid!=0 or st.st_mode&0o022 or st.st_size>limit:raise ValueError("untrusted updater file")
        with os.fdopen(fd,"rb",closefd=False) as f:return f.read(limit+1)
    finally:os.close(fd)
def release_tag(version,distribution="rust"):
    if distribution!="rust":raise ValueError("invalid distribution")
    return "v"+version
def package_name(version,distribution="rust"):
    return "yuji-probe-"+("rust" if distribution=="rust" else "panel")+"-"+version+".zip"
def allowed(url,version,distribution="rust"):
    u=urllib.parse.urlsplit(url)
    if u.scheme!="https" or u.username or u.password or u.port not in (None,443) or u.fragment:return False
    if u.hostname=="github.com":return u.path.startswith(f"/{REPOSITORY}/releases/download/{release_tag(version,distribution)}/") and not u.query
    return ((u.hostname=="release-assets.githubusercontent.com" and u.path.startswith("/github-production-release-asset/"))
      or(u.hostname=="objects.githubusercontent.com" and u.path.startswith("/github-production-release-asset-2e65be/")))
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs):return None
def download(version,name,limit,cache=None,distribution="rust"):
    if cache:
        path=pathlib.Path(cache)/name
        if path.is_file():return trusted_file(path,limit)
    url=f"https://github.com/{REPOSITORY}/releases/download/{release_tag(version,distribution)}/{name}"
    opener=urllib.request.build_opener(NoRedirect,urllib.request.ProxyHandler({}))
    started=time.monotonic()
    for _ in range(5):
        if not allowed(url,version,distribution):raise ValueError("untrusted release redirect")
        try:r=opener.open(urllib.request.Request(url,headers={"User-Agent":"Yuji-Probe-Updater"}),timeout=20)
        except urllib.error.HTTPError as e:
            if e.code not in (301,302,303,307,308):raise
            url=urllib.parse.urljoin(url,e.headers["Location"]);continue
        with r:
            if r.status!=200:raise ValueError("release unavailable")
            size=r.headers.get("Content-Length")
            if size and int(size)>limit:raise ValueError("release too large")
            out=bytearray()
            while True:
                if time.monotonic()-started>180:raise ValueError("download timeout")
                chunk=r.read(65536)
                if not chunk:return bytes(out)
                out.extend(chunk)
                if len(out)>limit:raise ValueError("release too large")
    raise ValueError("too many release redirects")
def verify(manifest,version,tmp,distribution="rust"):
    e=json.loads(manifest);payload=base64.b64decode(e["payload"],validate=True);sig=base64.b64decode(e["signature"],validate=True)
    if len(payload)>16384 or len(sig)!=64:raise ValueError("invalid signed manifest")
    (tmp/"key.der").write_bytes(bytes.fromhex("302a300506032b6570032100")+base64.b64decode(PUBLIC))
    (tmp/"payload").write_bytes(payload);(tmp/"signature").write_bytes(sig)
    subprocess.run(["openssl","pkeyutl","-verify","-pubin","-inkey",str(tmp/"key.der"),"-keyform","DER","-rawin","-in",str(tmp/"payload"),"-sigfile",str(tmp/"signature")],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    m=json.loads(payload);f=m["files"]["panel"]
    if m["version"]!=version or f["name"]!=package_name(version,distribution) or not re.fullmatch("[a-f0-9]{64}",f["sha256"]) or not 1024<=f["size"]<=64*1024*1024:raise ValueError("invalid release metadata")
    return f
def unpack(raw,version,tmp,distribution="rust"):
    archive=tmp/"release.zip";archive.write_bytes(raw);target=tmp/"stage";target.mkdir()
    prefix=package_name(version,distribution)[:-4]+"/";total=0
    with zipfile.ZipFile(archive) as z:
        entries=z.infolist()
        if len(entries)>12000:raise ValueError("too many archive entries")
        seen=set()
        for info in entries:
            name=info.filename
            if not name.startswith(prefix) or "\\" in name or "\0" in name:raise ValueError("archive path rejected")
            name=name[len(prefix):]
            if not name:continue
            pure=pathlib.PurePosixPath(name)
            if pure.is_absolute() or any(x in ("..",".","") for x in name.rstrip("/").split("/")) or name.rstrip("/") in seen:raise ValueError("archive path rejected")
            seen.add(name.rstrip("/"));mode=info.external_attr>>16
            if stat.S_ISLNK(mode) or (stat.S_IFMT(mode) not in (0,stat.S_IFREG,stat.S_IFDIR)):raise ValueError("archive type rejected")
            total+=info.file_size
            if total>512*1024*1024 or info.file_size>64*1024*1024:raise ValueError("archive size rejected")
            dest=target.joinpath(*pure.parts)
            if info.is_dir():dest.mkdir(parents=True,exist_ok=True);continue
            if pure.parts[0]=="storage" and name!="storage/.gitkeep":raise ValueError("release contains private state")
            dest.parent.mkdir(parents=True,exist_ok=True)
            with z.open(info) as source,dest.open("xb") as f:shutil.copyfileobj(source,f,65536)
            dest.chmod(0o755 if pure.parts[0]=="bin" or name.endswith(".sh") else 0o644)
    for directory,_,_ in os.walk(target):os.chmod(directory,0o755)
    required=["bin/probe-linux-amd64","ops/templates/nginx-rust.conf"] if distribution=="rust" else ["app/bootstrap.php","public/index.php","bin/probe-linux-amd64"]
    for name in required:
        if not (target/name).is_file():raise ValueError("release is incomplete")
    return target
def relabel(cfg):
    if pathlib.Path("/sys/fs/selinux/enforce").exists():
        subprocess.run(["/usr/sbin/restorecon","-RF",cfg["root"],str(pathlib.Path(cfg["state"]).parent)],check=True,timeout=90,stdout=subprocess.DEVNULL)
def service(cfg,action):subprocess.run(["systemctl",action,cfg["service"]],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=45)
def ownership(path,uid,gid):
    root=os.open(path,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    def visit(fd):
        st=os.fstat(fd)
        if st.st_uid not in (0,uid):raise ValueError("unexpected private state owner")
        os.fchown(fd,uid,gid);os.fchmod(fd,0o700)
        for entry in os.scandir(fd):
            info=entry.stat(follow_symlinks=False)
            if not(stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):raise ValueError("unsafe private state entry")
            flags=os.O_RDONLY|os.O_NOFOLLOW|(os.O_DIRECTORY if stat.S_ISDIR(info.st_mode) else os.O_NONBLOCK)
            child=os.open(entry.name,flags,dir_fd=fd)
            try:
                actual=os.fstat(child)
                if actual.st_ino!=info.st_ino or actual.st_dev!=info.st_dev or actual.st_uid not in (0,uid):raise ValueError("private state changed")
                if stat.S_ISDIR(actual.st_mode):visit(child)
                else:
                    if actual.st_nlink!=1:raise ValueError("hard-linked private state rejected")
                    os.fchown(child,uid,gid);os.fchmod(child,0o600)
            finally:os.close(child)
    try:visit(root)
    finally:os.close(root)
def safe_snapshot(source,dest,uid,allow_owner=None):
    # Read private files through directory descriptors, without following links.
    owners={0,uid}
    if allow_owner is not None:owners.add(allow_owner)
    fd=os.open(source,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    def copy_dir(current,target):
        target.mkdir(mode=0o700)
        st=os.fstat(current)
        if st.st_uid not in owners:raise ValueError("unexpected state owner")
        for entry in os.scandir(current):
            info=entry.stat(follow_symlinks=False)
            if info.st_uid not in owners or not(stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):raise ValueError("unsafe state entry")
            flags=os.O_RDONLY|os.O_NOFOLLOW|(os.O_DIRECTORY if stat.S_ISDIR(info.st_mode) else 0)
            child=os.open(entry.name,flags,dir_fd=current)
            try:
                actual=os.fstat(child)
                if actual.st_ino!=info.st_ino or actual.st_dev!=info.st_dev:raise ValueError("state changed while copying")
                if stat.S_ISDIR(actual.st_mode):copy_dir(child,target/entry.name)
                else:
                    if actual.st_nlink!=1 or actual.st_size>256*1024*1024:raise ValueError("unsafe state file")
                    with os.fdopen(os.dup(child),"rb") as src,(target/entry.name).open("xb") as out:shutil.copyfileobj(src,out,65536)
                    (target/entry.name).chmod(0o600)
            finally:os.close(child)
    try:copy_dir(fd,dest)
    finally:os.close(fd)
def replace_contents(target,source):
    for entry in target.iterdir():
        if entry.is_dir() and not entry.is_symlink():shutil.rmtree(entry)
        else:entry.unlink()
    for entry in source.iterdir():os.rename(entry,target/entry.name)
def check(cfg,version):
    # PHP-FPM may not have created its socket when systemctl start returns.
    # Retry the wake request as well as the direct controller health probe.
    parsed=urllib.parse.urlsplit(cfg["origin"])
    class LocalHTTPS(http.client.HTTPSConnection):
        def connect(self):
            raw=socket.create_connection(("127.0.0.1",self.port),timeout=self.timeout)
            self.sock=self._context.wrap_socket(raw,server_hostname=self.host)
    key=(pathlib.Path(cfg["state"])/"app.key").read_bytes()
    gateway=hashlib.sha256(b"vistart-probe-php-gateway-v1:"+key).hexdigest()
    for attempt in range(35):
        if attempt%3==0 and parsed.scheme=="https":
            connection=LocalHTTPS(parsed.hostname,parsed.port or 443,timeout=8,context=ssl.create_default_context())
            try:
                connection.request("GET","/api/session");connection.getresponse().read(1024)
            except Exception:pass
            finally:connection.close()
        try:
            r=urllib.request.urlopen(urllib.request.Request("http://"+cfg["listen"]+"/_internal/health",headers={"X-Probe-Gateway":gateway}),timeout=2)
            with r:data=json.load(r)
            if data.get("ok") and data.get("origin")==cfg["origin"] and (version is None or data.get("version")==version):return
        except (OSError,ValueError):pass
        time.sleep(1)
    raise ValueError("updated controller failed health check")

def install_helper(args):
    select_layout(args.root)
    if not re.fullmatch("[a-z0-9-]{1,40}",args.name):raise ValueError("invalid instance name")
    root=pathlib.Path(args.root).resolve();state=pathlib.Path(args.state).resolve()
    if state.name!="control" or state.parent in (pathlib.Path("/"),pathlib.Path("/var"),pathlib.Path("/var/lib"),pathlib.Path("/srv"),pathlib.Path("/opt")) or state.parent==root:raise ValueError("state must be a dedicated instance directory ending in /control")
    auth=state/"auth.json";owner=auth.stat()
    if not root.is_dir() or not auth.is_file() or not (root/("bin/probe-linux-amd64" if args.distribution=="rust" else "public/index.php")).is_file():raise ValueError("panel is not installed")
    if not re.fullmatch("[a-zA-Z0-9_.@-]+\\.service",args.service):raise ValueError("invalid service")
    origin=urllib.parse.urlsplit(args.origin)
    if origin.scheme not in ("http","https") or not origin.hostname or origin.username or origin.password or origin.query or origin.fragment or origin.path not in ("","/"):raise ValueError("invalid HTTPS origin")
    if not re.fullmatch("127\\.0\\.0\\.1:[0-9]{1,5}",args.listen):raise ValueError("updater health address must be loopback")
    private(BASE);private(CONF);LIB.mkdir(parents=True,exist_ok=True,mode=0o755);LIB.chmod(0o755)
    dest=LIB/"update-panel.py";atomic(dest,pathlib.Path(__file__).read_bytes(),mode=0o755)
    cfg={"name":args.name,"root":str(root),"state":str(state),"service":args.service,"origin":args.origin.rstrip("/"),"listen":args.listen,"uid":owner.st_uid,"gid":owner.st_gid}
    cfg["distribution"]=args.distribution
    cp=CONF/(args.name+".json");atomic(cp,cfg)
    unit=("YJ-update-" if root==pathlib.Path("/opt/YJ") else "yuji-probe-rust-update-")+args.name
    if any("\n" in str(v) or "%" in str(v) for v in cfg.values()):raise ValueError("invalid instance configuration")
    system=pathlib.Path("/etc/systemd/system")
    (system/(unit+".service")).write_text("[Unit]\nDescription=Yuji Probe signed update ("+args.name+")\n[Service]\nType=oneshot\nExecStart=/usr/bin/python3 "+str(dest)+" --run "+str(cp)+"\nUMask=0077\nTimeoutStartSec=600\n")
    (system/(unit+".path")).write_text("[Unit]\nDescription=Yuji Probe update requests ("+args.name+")\n[Path]\nPathExists="+str(state/"update-request.json")+"\nUnit="+unit+".service\n[Install]\nWantedBy=multi-user.target\n")
    atomic(state/"updater-enabled",b"1\n",owner.st_uid,owner.st_gid)
    subprocess.run(["systemctl","daemon-reload"],check=True);subprocess.run(["systemctl","enable","--now",unit+".path"],check=True,stdout=subprocess.DEVNULL)
    print("Signed updater installed for",args.name)
def apply(cfg,request,cache=None):
    select_layout(cfg["root"])
    work=BASE/cfg["name"];private(work);root=pathlib.Path(cfg["root"]);state=pathlib.Path(cfg["state"])
    prior=work/"previous-program";priorstate=work/"previous-state";meta=work/"previous.json"
    distribution=cfg.get("distribution","rust")
    release_tag("0.0.0",distribution)
    action=request["action"];version=request.get("version","")
    if action not in ("update","rollback") or (action=="update" and not VERSION.fullmatch(version)):raise ValueError("invalid update request")
    if time.time()-request.get("at",0)>900 or request.get("at",0)>time.time()+60:raise ValueError("update request expired")
    with tempfile.TemporaryDirectory(prefix=".yuji-update-",dir=root.parent) as folder:
        tmp=pathlib.Path(folder);tmp.chmod(0o700)
        if action=="update":
            manifest=download(version,"panel-stable.json",16384,cache,distribution);f=verify(manifest,version,tmp,distribution)
            raw=download(version,f["name"],64*1024*1024,cache,distribution)
            if len(raw)!=f["size"] or hashlib.sha256(raw).hexdigest()!=f["sha256"]:raise ValueError("release checksum mismatch")
            target=unpack(raw,version,tmp,distribution);restore_state=None
        else:
            if not prior.is_dir() or not priorstate.is_dir():raise ValueError("no rollback snapshot")
            version=json.loads(trusted_file(meta,2048)).get("version")
            target=tmp/"stage";shutil.copytree(prior,target,ignore=shutil.ignore_patterns("storage"))
            restore_state=priorstate
        if action=="rollback" and root==pathlib.Path("/opt/YJ") and tuple(map(int,version.split(".")))<(0,11,5):
            for name in ("manage.sh","ops/manage.py","ops/install-command.py","ops/prepare-service.py","ops/update-panel.py","ops/nginx_setup.py","ops/templates/YJ-direct.service","ops/templates/YJ.service"):
                src=root/name
                if not src.is_file() or src.is_symlink():raise ValueError("YJ compatibility helper is unavailable")
                dest=target/name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(src,dest)
        for local in ("php-fpm.production.conf","php-fpm.test.conf","nginx-test.conf"):
            existing=root/"ops"/local
            if existing.is_file() and not existing.is_symlink():
                shutil.copy2(existing,target/"ops"/local)
        if shutil.disk_usage(root.parent).free<512*1024*1024:raise ValueError("at least 512 MiB free space is required")
        oldversion=subprocess.check_output([str(root/"bin"/("probe-linux-arm64" if os.uname().machine=="aarch64" else "probe-linux-amd64")),"--version"],text=True,timeout=10).split()[1]
        live_storage=root/"storage";data_parent=state.parent
        internal=data_parent==live_storage
        if root in data_parent.parents and not internal:raise ValueError("unsupported private state layout")
        stopped=False;swapped=False;root_moved=False;data_moved=False;snapshot_complete=False;old=tmp/"old"
        try:
            service(cfg,"stop");stopped=True
            safe_snapshot(data_parent,tmp/"state-snapshot",cfg["uid"]);snapshot_complete=True
            if restore_state:
                replacement=tmp/"restore-state";safe_snapshot(restore_state,replacement,0,allow_owner=cfg["uid"])
                ownership(replacement,cfg["uid"],cfg["gid"])
                replace_contents(data_parent,replacement)
            ownership(data_parent,cfg["uid"],cfg["gid"])
            if (target/"storage").exists():shutil.rmtree(target/"storage")
            if not internal:(target/"storage").symlink_to(data_parent,target_is_directory=True)
            os.rename(root,old);root_moved=True
            if internal:os.rename(old/"storage",target/"storage");data_moved=True
            os.rename(target,root);swapped=True
            relabel(cfg)
            service(cfg,"start");stopped=False
            check(cfg,version)
            if prior.exists():shutil.rmtree(prior)
            if priorstate.exists():shutil.rmtree(priorstate)
            shutil.copytree(old,prior,ignore=shutil.ignore_patterns("storage"))
            shutil.copytree(tmp/"state-snapshot",priorstate)
            atomic(meta,{"version":oldversion})
        except Exception:
            if swapped:
                try:service(cfg,"stop")
                except Exception:pass
                failed=tmp/"failed";os.rename(root,failed)
                if internal:os.rename(failed/"storage",old/"storage")
                os.rename(old,root)
            elif root_moved:
                if data_moved:os.rename(target/"storage",old/"storage")
                os.rename(old,root)
            if snapshot_complete:
                replace_contents(data_parent,tmp/"state-snapshot")
                ownership(data_parent,cfg["uid"],cfg["gid"])
            if stopped or swapped:
                try:relabel(cfg);service(cfg,"start")
                except Exception:pass
            raise
    return version
def run(args):
    cfgpath=pathlib.Path(args.run);cfg=json.loads(trusted_file(cfgpath,8192));select_layout(cfg["root"]);work=BASE/cfg["name"];private(work)
    with (work/"lock").open("a") as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        state=pathlib.Path(cfg["state"]);path=state/"update-request.json"
        if not path.exists():return
        # Consume invalid requests too, so systemd.path cannot spin on a rejected file.
        try:
            fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
            try:
                st=os.fstat(fd)
                if not stat.S_ISREG(st.st_mode) or st.st_nlink!=1 or st.st_size>2048 or st.st_uid!=cfg["uid"]:raise ValueError("invalid request owner")
                request=json.loads(os.read(fd,2049))
            finally:os.close(fd)
        finally:
            try:path.unlink()
            except FileNotFoundError:pass
        def report(message,status):
            actual=pathlib.Path(cfg["state"])
            atomic(actual/"update-result.json",{"state":status,"message":message,"at":int(time.time())},cfg["uid"],cfg["gid"])
        report("正在验证发布包并准备更新","running")
        try:
            version=apply(cfg,request,args.release_dir);report("操作完成，当前版本 "+str(version),"done")
        except Exception as e:
            report("操作未完成，已保留或恢复原版本："+str(e)[:200],"failed")
            raise
def main():
    if os.geteuid()!=0:raise SystemExit("Run this updater as root")
    os.umask(0o077)
    a=argparse.ArgumentParser();a.add_argument("--configure",action="store_true");a.add_argument("--name");a.add_argument("--root");a.add_argument("--state");a.add_argument("--service");a.add_argument("--origin");a.add_argument("--listen",default="127.0.0.1:19281");a.add_argument("--run");a.add_argument("--release-dir");a.add_argument("--distribution",choices=["rust"],default="rust")
    args=a.parse_args()
    if args.configure:install_helper(args)
    elif args.run:run(args)
    else:a.error("choose --configure or --run")
if __name__=="__main__":main()
