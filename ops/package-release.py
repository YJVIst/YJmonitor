#!/usr/bin/env python3
"""Create a clean signed installation ZIP. Signing key stays outside this tree."""
import argparse, base64, hashlib, json, os, pathlib, shutil, stat, zipfile
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization

parser=argparse.ArgumentParser()
parser.add_argument("--variant", choices=["rust"], default="rust")
signing=parser.add_mutually_exclusive_group(required=True)
signing.add_argument("--signing-key",type=pathlib.Path)
signing.add_argument("--prepare-only",action="store_true",help="Prepare artifacts and unsigned manifest payloads for offline signing")
parser.add_argument("--output",type=pathlib.Path,required=True)
parser.add_argument("--agent-amd64",type=pathlib.Path,required=True)
parser.add_argument("--agent-arm64",type=pathlib.Path,required=True)
args=parser.parse_args()
root=pathlib.Path(__file__).resolve().parent.parent
version="0.11.9"
flavor="rust" if args.variant=="rust" else "panel"
out=args.output.resolve()
if out==root or root in out.parents: raise SystemExit("Release output must be outside the source tree")
anchor=(root/"source/controller-rust/assets/release-public.txt").read_text().strip()
if args.prepare_only and out.exists() and any(out.iterdir()):
    raise SystemExit("Unsigned staging requires an empty output directory")
if args.signing_key:
    keypath=args.signing_key.resolve()
    if keypath==root or root in keypath.parents: raise SystemExit("Signing key must be outside source tree")
    if stat.S_IMODE(keypath.stat().st_mode)&0o077: raise SystemExit("Signing key must have private permissions")
    raw=keypath.read_bytes()
    if len(raw) not in (32,64): raise SystemExit("Expected raw Ed25519 private key")
    key=Ed25519PrivateKey.from_private_bytes(raw[:32])
    public=key.public_key().public_bytes(serialization.Encoding.Raw,serialization.PublicFormat.Raw)
    if base64.b64encode(public).decode()!=anchor: raise SystemExit("Signing key does not match embedded public key")
files={}
for section in ["app","bin","docs","ops","public","source","THIRD-PARTY-NOTICES"]:
    for p in sorted((root/section).rglob("*")):
        rel=p.relative_to(root)
        if any(x in (".git","target","node_modules","__pycache__") for x in rel.parts): continue
        if rel.parts[:2] in (("source","state"),("source","public")): continue
        if p.is_symlink(): raise SystemExit("Package cannot include symbolic links")
        if p.is_file():
            if args.variant=="rust" and (p.suffix==".php" or p.name=="php-pool.conf"):continue
            if p.suffix==".log": continue
            if p.suffix in (".log",".pyc",".zip") or p.name in (".env","signing.key"): raise SystemExit("Unexpected runtime/private file: "+str(rel))
            files[str(rel)]=p.read_bytes()
for name in ["README.md","LICENSE","SECURITY.md","install.sh","manage.sh","migrate-repository.sh","migrate-signing-key.sh",".gitignore",".gitattributes"]:
    files[name]=(root/name).read_bytes()
files["storage/.gitkeep"]=b""
sums={k:hashlib.sha256(v).hexdigest() for k,v in files.items()}
files["SHA256SUMS.json"]=(json.dumps(sums,ensure_ascii=False,indent=2)+"\n").encode()
out.mkdir(parents=True,exist_ok=True)
name=f"yuji-probe-{flavor}-{version}.zip"
archive=out/name
with zipfile.ZipFile(archive,"w",zipfile.ZIP_DEFLATED,compresslevel=9) as z:
    # An explicit private storage directory is important for manual extraction.
    d=zipfile.ZipInfo(f"yuji-probe-{flavor}-{version}/storage/",(2026,9,30,0,0,0))
    d.external_attr=(stat.S_IFDIR|0o700)<<16
    z.writestr(d,b"")
    for rel,data in sorted(files.items()):
        entry=zipfile.ZipInfo(f"yuji-probe-{flavor}-{version}/{rel}",(2026,9,30,0,0,0))
        mode=0o755 if rel.startswith("bin/") or rel=="install.sh" or rel.endswith(".sh") else 0o644
        if rel=="storage/.gitkeep":mode=0o600
        entry.external_attr=(stat.S_IFREG|mode)<<16
        entry.compress_type=zipfile.ZIP_DEFLATED
        z.writestr(entry,data)
entry={"name":name,"sha256":hashlib.sha256(archive.read_bytes()).hexdigest(),"size":archive.stat().st_size}
(out/(name+".sha256")).write_text(entry["sha256"]+"  "+name+"\n")
def sign(name,version,entries):
    payload=json.dumps({"version":version,"files":entries},separators=(",",":"),sort_keys=True).encode()
    if args.prepare_only:
        (out/(name+".payload")).write_bytes(payload)
        return
    envelope={"payload":base64.b64encode(payload).decode(),"signature":base64.b64encode(key.sign(payload)).decode()}
    (out/name).write_text(json.dumps(envelope,separators=(",",":"))+"\n")
sign("panel-stable.json",version,{"panel":entry})
controllers={}
for arch in ["amd64","arm64"]:
    name=f"vistart-probe-controller-{version}-linux-{arch}"
    path=out/name
    shutil.copyfile(root/"bin"/("probe-linux-"+arch),path)
    path.chmod(0o755)
    controllers[arch]={"name":name,"size":path.stat().st_size,"sha256":hashlib.sha256(path.read_bytes()).hexdigest()}
sign("controller-stable.json",version,controllers)
agents={}
for arch in ["amd64","arm64"]:
    path=getattr(args,"agent_"+arch).resolve()
    raw=path.read_bytes()
    expected_machine=62 if arch=="amd64" else 183
    if raw[:4]!=b"\x7fELF" or int.from_bytes(raw[18:20],"little")!=expected_machine:
        raise SystemExit("Agent is not the expected ELF architecture")
    name=f"vistart-probe-agent-0.2.5-linux-{arch}"
    shutil.copyfile(path,out/name)
    (out/name).chmod(0o755)
    agents[arch]={"name":name,"size":len(raw),"sha256":hashlib.sha256(raw).hexdigest()}
sign("stable.json","0.2.5",agents)
(out/"Agent-SHA256SUMS").write_text("".join(entry["sha256"]+"  "+entry["name"]+"\n" for entry in agents.values()))

(out/"release-public.txt").write_text(anchor+"\n")
shutil.copyfile(root/"install.sh",out/"install.sh")
shutil.copyfile(root/"migrate-signing-key.sh",out/"migrate-signing-key.sh")
shutil.copyfile(root/"ops/agent-release.py",out/"agent-release.py")
print(json.dumps({"package":entry,"source_files":len(files),"controllers":controllers},indent=2))
