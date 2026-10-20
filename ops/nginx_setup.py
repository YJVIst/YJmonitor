#!/usr/bin/env python3
"""Optional root-only HTTPS provisioning. Never edits a foreign virtual host."""
import fnmatch
import getpass
import hashlib
import http.client
import ipaddress
import json
import os
import pathlib
import re
import secrets
import shlex
import signal
import socket
import ssl
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

LEGACY = pathlib.Path(__file__).resolve().parents[1] == pathlib.Path('/opt/yuji-probe')
NAME = 'yuji-probe' if LEGACY else 'YJ'
ROOT = pathlib.Path('/opt') / NAME
CONF = pathlib.Path('/etc') / NAME / 'https'
WORK = pathlib.Path('/var/lib') / (NAME + '-https')
WEBROOT = pathlib.Path('/var/lib') / (NAME + '-acme')
VENV = pathlib.Path('/opt') / (NAME + '-certbot')
SYSTEMD = pathlib.Path('/etc/systemd/system')
SITE_NAME = NAME + '-managed.conf'
TIMER = NAME + '-https-renew.timer'
MARKER = '# Managed by Yuji Probe HTTPS. See docs/自动HTTPS.md.\n'
ACME_SERVER = 'https://acme-v02.api.letsencrypt.org/directory'
NGINX_KEYS = {
    '573BFD6B3D8FBC641079A6ABABF5BD827BD9BF62',
    '8540A6F18833A80E9C1653A42FD21310B49F6B46',
    '9E9BE90EACBCDE69FE9B204CBCDCD8A38D88A2B3',
}
# Official Cloudflare ranges; only these immediate peers may supply client IPs.
CF_RANGES = (
    '173.245.48.0/20', '103.21.244.0/22', '103.22.200.0/22',
    '103.31.4.0/22', '141.101.64.0/18', '108.162.192.0/18',
    '190.93.240.0/20', '188.114.96.0/20', '197.234.240.0/22',
    '198.41.128.0/17', '162.158.0.0/15', '104.16.0.0/13',
    '104.24.0.0/14', '172.64.0.0/13', '131.0.72.0/22',
    '2400:cb00::/32', '2606:4700::/32', '2803:f800::/32',
    '2405:b500::/32', '2405:8100::/32', '2a06:98c0::/29',
    '2c0f:f248::/32',
)


def run(args, **kwargs):
    return subprocess.run([str(x) for x in args], check=True, timeout=kwargs.pop('timeout', 120), **kwargs)


def capture(args, **kwargs):
    return run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, **kwargs).stdout


def trusted(path, directory=False):
    path = pathlib.Path(path)
    info = path.lstat()
    expected = stat.S_ISDIR if directory else stat.S_ISREG
    if not expected(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
        raise ValueError('需要 root 所有、不可被其他用户写入的路径：' + str(path))
    return path


def parents_safe(path):
    for parent in reversed((path, *path.parents)):
        trusted(parent, directory=True)


def directory(path, mode=0o700):
    if not path.exists():
        parents_safe(path.parent)
        path.mkdir(mode=mode)
        path.chmod(mode)
    parents_safe(path)
    return path


def write(path, data, mode=0o600):
    parents_safe(path.parent)
    if path.exists() or path.is_symlink():
        trusted(path)
    raw = data.encode() if isinstance(data, str) else data
    fd, temporary = tempfile.mkstemp(prefix='.yuji-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            os.fchmod(stream.fileno(), mode)
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def read_json(path):
    parents_safe(path.parent)
    trusted(path)
    if path.stat().st_size > 131072:
        raise ValueError('配置文件过大')
    return json.loads(path.read_text())


def domain_name(value):
    value = value.strip()
    if not value or any(c.isspace() for c in value) or any(c in value for c in '/\\:@*?#%\'"'):
        raise ValueError('只填写已解析的域名，例如 probe.example.com，不含协议、端口或路径')
    try:
        value = value.encode('idna').decode('ascii').lower()
    except UnicodeError as exc:
        raise ValueError('域名格式不正确') from exc
    labels = value.split('.')
    if len(value) > 253 or len(labels) < 2 or not re.fullmatch(r'[a-z][a-z0-9-]*', labels[-1]):
        raise ValueError('请使用完整的公网域名')
    if any(not re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', label) for label in labels):
        raise ValueError('域名格式不正确')
    return value


def resolved(domain):
    addresses = {item[4][0] for item in socket.getaddrinfo(domain, 80, type=socket.SOCK_STREAM)}
    if not addresses or any(not ipaddress.ip_address(value).is_global for value in addresses):
        raise ValueError('域名尚未解析到公网地址，请检查 A / AAAA 记录')
    # Orange-cloud DNS correctly returns Cloudflare edge IPs, not the origin IP.
    return addresses


def safe_nginx_path(value):
    if not re.fullmatch(r'/[a-zA-Z0-9_./+-]+', str(value)) or '..' in pathlib.Path(value).parts:
        raise ValueError('Nginx 路径含不支持的字符：' + str(value))
    return pathlib.Path(value)


def platform():
    values = {}
    for line in pathlib.Path('/etc/os-release').read_text().splitlines():
        if '=' in line:
            key, value = line.split('=', 1)
            words = shlex.split(value)
            if len(words) == 1:
                values[key] = words[0]
    return values


def download(url, limit=131072):
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            raise ValueError('官方资源出现意外重定向')
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with client.open(url, timeout=30) as response:
        raw = response.read(limit + 1)
        if response.status != 200 or len(raw) > limit:
            raise ValueError('官方资源下载失败或超出大小限制')
        return raw


def apt_install(*packages):
    env = dict(os.environ, DEBIAN_FRONTEND='noninteractive')
    run(['apt-get', 'update', '-qq'], env=env, timeout=300)
    run(['apt-get', 'install', '-y', '--no-install-recommends', *packages], env=env, timeout=600)


def verified_nginx_key():
    raw = download('https://nginx.org/keys/nginx_signing.key')
    with tempfile.TemporaryDirectory(prefix='nginx-key-', dir=WORK) as temp:
        path = pathlib.Path(temp) / 'signing.asc'
        path.write_bytes(raw)
        text = capture(['gpg', '--batch', '--homedir', temp, '--with-colons', '--show-keys', path])
        primary, want = set(), False
        for line in text.splitlines():
            fields = line.split(':')
            if fields[0] == 'pub':
                want = True
            elif fields[0] == 'sub':
                want = False
            elif fields[0] == 'fpr' and want:
                primary.add(fields[9])
                want = False
        if not primary or not primary <= NGINX_KEYS:
            raise ValueError('Nginx 签名密钥指纹不匹配，请更新脚本后重试')
        binary = run(['gpg', '--batch', '--homedir', temp, '--dearmor'],
                     input=raw, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout
        return raw, binary


def reserved_write(path, content, mode=0o644):
    if path.exists() or path.is_symlink():
        trusted(path)
        if path.read_bytes() != (content.encode() if isinstance(content, str) else content):
            raise ValueError('已有不同配置，未覆盖：' + str(path))
    else:
        write(path, content, mode)


def install_nginx():
    transaction = Transaction()
    def repository(path, content, mode=0o644):
        if path.exists() or path.is_symlink():
            reserved_write(path, content, mode)
        else:
            transaction.put(path, content, mode)
    try:
        _install_nginx(repository)
    except BaseException:
        transaction.rollback()
        raise


def _install_nginx(repository):
    values = platform()
    distro, version = values.get('ID', ''), values.get('VERSION_ID', '')
    print('\n  未安装 Nginx，正在安装最新可用的签名预编译包…')
    if distro in ('debian', 'ubuntu'):
        codename = {
            ('debian', '12'): 'bookworm', ('debian', '13'): 'trixie',
            ('ubuntu', '22.04'): 'jammy', ('ubuntu', '24.04'): 'noble',
            ('ubuntu', '26.04'): 'resolute',
        }.get((distro, version))
        if not codename:
            raise ValueError('此系统没有受支持的 Nginx 官方包，请先手动安装 Nginx')
        apt_install('ca-certificates', 'gnupg')
        _, binary = verified_nginx_key()
        key = pathlib.Path('/usr/share/keyrings/yuji-nginx.gpg')
        repository(key, binary)
        repository(pathlib.Path('/etc/apt/sources.list.d/yuji-nginx.list'),
                       'deb [signed-by=' + str(key) + '] https://nginx.org/packages/' + distro + ' ' + codename + ' nginx\n')
        repository(pathlib.Path('/etc/apt/preferences.d/yuji-nginx'),
                       'Package: nginx nginx-module-*\nPin: origin nginx.org\nPin-Priority: 900\n')
        apt_install('nginx')
    elif distro in ('rocky', 'almalinux', 'centos') and version.split('.')[0] in ('9', '10'):
        run(['dnf', 'install', '-y', 'ca-certificates', 'gnupg2'], timeout=600)
        raw, _ = verified_nginx_key()
        key = pathlib.Path('/etc/pki/rpm-gpg/RPM-GPG-KEY-yuji-nginx')
        repository(key, raw)
        run(['rpmkeys', '--import', key])
        content = ('[yuji-nginx-stable]\nname=nginx official stable\nbaseurl=https://nginx.org/packages/centos/'
                   + version.split('.')[0] + '/$basearch/\nenabled=1\ngpgcheck=1\nsslverify=1\nmodule_hotfixes=true\n'
                   + 'gpgkey=file://' + str(key) + '\n')
        repository(pathlib.Path('/etc/yum.repos.d/yuji-nginx.repo'), content)
        # Restrict nginx itself to its official repository; dependencies use distro repositories.
        run(['dnf', 'install', '-y', '--refresh', '--setopt=*.excludepkgs=nginx',
             '--setopt=yuji-nginx-stable.excludepkgs=', 'nginx'], timeout=600)
    elif distro == 'fedora' and version in ('42', '43', '44'):
        print('  Fedora 使用本发行版官方仓库的最新 Nginx 二进制包。')
        run(['dnf', 'install', '-y', '--refresh', 'nginx'], timeout=600)
    else:
        raise ValueError('此系统没有受支持的 Nginx 包，请先手动安装 Nginx')
    print('  Nginx 安装完成。')


def masters():
    found = []
    for proc in pathlib.Path('/proc').iterdir():
        if not proc.name.isdigit():
            continue
        try:
            title = (proc / 'cmdline').read_bytes().split(b'\0', 1)[0].decode()
            if title.startswith('nginx: master process ') and proc.stat().st_uid == 0:
                found.append((int(proc.name), str((proc / 'exe').resolve(strict=True)), title[22:]))
        except (OSError, UnicodeError):
            continue
    return found


def nginx_args(title):
    words = shlex.split(title)
    args, i = [], 1
    while i < len(words):
        option = words[i]
        if option in ('-p', '-c', '-e') and i + 1 < len(words):
            safe_nginx_path(words[i + 1])
            args += [option, words[i + 1]]
            i += 2
        elif option == '-g':
            i += 1
            start = i
            while i < len(words) and not words[i].startswith('-'):
                i += 1
            args += ['-g', ' '.join(words[start:i])]
        else:
            raise ValueError('无法安全识别现有 Nginx 启动参数，请使用手动反向代理文档')
    return args


def discover_nginx(install=True):
    running = masters()
    if len(running) > 1:
        raise ValueError('本机有多个 Nginx 主进程，请使用手动反向代理文档指定实例')
    if running:
        _, binary, title = running[0]
        args = nginx_args(title)
    else:
        candidates = []
        for path in ('/www/server/nginx/sbin/nginx', '/usr/sbin/nginx', '/usr/local/nginx/sbin/nginx'):
            if pathlib.Path(path).is_file():
                real = str(pathlib.Path(path).resolve())
                if real not in candidates:
                    candidates.append(real)
        if not candidates:
            if not install:
                raise ValueError('Nginx 已不存在，未自动重新安装')
            busy_ports()
            install_nginx()
            return discover_nginx(False)
        if len(candidates) != 1:
            raise ValueError('发现多个 Nginx 安装，请先启动需要配置的实例')
        binary, args = candidates[0], []
    trusted(binary)
    parents_safe(pathlib.Path(binary).parent)
    version = run([binary, '-V'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True).stderr
    if '--with-http_ssl_module' not in version or '--with-http_realip_module' not in version:
        raise ValueError('现有 Nginx 需要 http_ssl 和 http_realip 模块')
    options = shlex.split(version.split('configure arguments:', 1)[-1])
    compiled = dict(arg[2:].split('=', 1) for arg in options if arg.startswith('--') and '=' in arg)
    prefix = args[args.index('-p') + 1] if '-p' in args else compiled.get('prefix', '/usr/local/nginx')
    config = args[args.index('-c') + 1] if '-c' in args else compiled.get('conf-path', 'conf/nginx.conf')
    if not pathlib.Path(config).is_absolute():
        config = str(pathlib.Path(prefix) / config)
    safe_nginx_path(config)
    safe_nginx_path(prefix)
    if '-c' not in args:
        args += ['-c', config]
    if '-p' not in args:
        args += ['-p', prefix]
    return {'binary': binary, 'args': args, 'config': config, 'prefix': prefix}


def nginx_check(nginx):
    result = run([nginx['binary'], *nginx['args'], '-T'], stdout=subprocess.PIPE,
                 stderr=subprocess.PIPE, text=True, timeout=20)
    if 'conflicting server name' in result.stderr:
        raise ValueError('Nginx 存在同名站点冲突，请先修正后重试')
    return result.stdout


def busy_ports():
    output = capture(['ss', '-H', '-ltnp', '( sport = :80 or sport = :443 )'])
    for line in output.splitlines():
        if '"nginx"' not in line:
            raise ValueError('80 或 443 端口正在被其他程序使用，未改动该程序')


def nginx_reload(nginx):
    nginx_check(nginx)
    found = masters()
    if len(found) == 1 and found[0][1] == nginx['binary']:
        os.kill(found[0][0], signal.SIGHUP)
    elif found:
        raise ValueError('Nginx 运行实例已变化，未执行重载')
    else:
        exec_start = capture(['systemctl', 'show', 'nginx.service', '-p', 'ExecStart', '--value'])
        if nginx['binary'] not in exec_start:
            raise ValueError('现有 Nginx 尚未启动，请先使用其原有管理方式启动后重试')
        run(['systemctl', 'enable', '--now', 'nginx.service'], stdout=subprocess.DEVNULL)


def tokens(text):
    """Fail closed on uncertain grammar; never modify the main configuration."""
    result, word, quote, i = [], '', None, 0
    while i < len(text):
        c = text[i]
        if c == '\\' and i + 1 < len(text):
            word += text[i:i + 2]
            i += 2
            continue
        if quote:
            if c == quote:
                quote = None
            else:
                word += c
        elif c in ('"', "'"):
            quote = c
        elif c == '#' and not word:
            end = text.find('\n', i)
            i = len(text) if end < 0 else end
            continue
        elif c == '$' and text[i:i + 2] == '$' + '{':
            end = text.find('}', i + 2)
            if end < 0:
                raise ValueError('无法解析 Nginx 变量')
            word += text[i:end + 1]
            i = end + 1
            continue
        elif c.isspace() or c in '{};':
            if word:
                result.append(word)
                word = ''
            if c in '{};':
                result.append(c)
        else:
            word += c
        i += 1
    if quote:
        raise ValueError('Nginx 配置引号未闭合')
    if word:
        result.append(word)
    return result


def directives(text):
    stream = iter(tokens(text))
    def block(nested=False):
        out, args = [], []
        for token in stream:
            if token == ';':
                if not args:
                    raise ValueError('无法解析 Nginx 指令')
                out.append((args, None))
                args = []
            elif token == '{':
                out.append((args, block(True)))
                args = []
            elif token == '}':
                if args or not nested:
                    raise ValueError('无法解析 Nginx 区块')
                return out
            else:
                args.append(token)
        if nested or args:
            raise ValueError('Nginx 配置不完整')
        return out
    return block()


def inventory(nginx, dump):
    files = {}
    pieces = re.split(r'^# configuration file (.+):\n', dump, flags=re.M)
    for index in range(1, len(pieces), 2):
        files[pathlib.Path(pieces[index])] = directives(pieces[index + 1])
    includes, servers = [], []

    def matching(pattern):
        if not pattern.is_absolute():
            pattern = pathlib.Path(nginx['config']).parent / pattern
        return [path for path in files if path.parent.resolve() == pattern.parent.resolve()
                and fnmatch.fnmatch(path.name, pattern.name)]

    def walk(items, context, source, chain):
        for args, children in items:
            if not args:
                continue
            if args[0] == 'include' and len(args) == 2:
                pattern = pathlib.Path(args[1])
                if not pattern.is_absolute():
                    pattern = pathlib.Path(nginx['config']).parent / pattern
                if context == ('http',) and pattern.name in ('*.conf', '*'):
                    includes.append(pattern.parent.resolve())
                for path in matching(pattern):
                    if path in chain or len(chain) > 32:
                        raise ValueError('Nginx include 循环或嵌套过深')
                    walk(files[path], context, path, chain + (path,))
            elif children is not None:
                if args == ['server'] and context == ('http',):
                    names = []
                    def collect(nodes, seen):
                        for directive, _ in nodes:
                            if directive[0] == 'server_name':
                                names.extend(directive[1:])
                            elif directive[0] == 'include' and len(directive) == 2:
                                for path in matching(pathlib.Path(directive[1])):
                                    if path not in seen:
                                        collect(files[path], seen | {path})
                    collect(children, {source})
                    servers.append((source.resolve(), names))
                walk(children, context + (args[0],), source, chain)
    main = pathlib.Path(nginx['config'])
    if main not in files:
        raise ValueError('无法读取 Nginx 主配置')
    walk(files[main], (), main, (main,))
    return includes, servers


def choose_site(nginx, domain, previous):
    includes, servers = inventory(nginx, nginx_check(nginx))
    owned = pathlib.Path(previous['site']) if previous else None
    if owned:
        safe_nginx_path(owned)
        trusted(owned)
        if owned.name != SITE_NAME or hashlib.sha256(owned.read_bytes()).hexdigest() != previous.get('site_sha256'):
            raise ValueError('自动站点配置已被手动修改，未覆盖；请按文档核对后处理')
    for source, names in servers:
        if source == owned:
            continue
        for name in names:
            value = name.lower()
            if value.startswith('~') or value == domain or fnmatch.fnmatch(domain, value) or (
                    value.startswith('.') and (domain == value[1:] or domain.endswith(value))):
                raise ValueError('此域名可能已由其他站点使用，未覆盖：' + str(source))
    if owned:
        if owned.parent not in includes:
            raise ValueError('原自动站点已不在 Nginx http include 中')
        return owned
    for folder in includes:
        if folder.is_dir():
            parents_safe(folder)
            target = safe_nginx_path(folder / SITE_NAME)
            if target.exists() or target.is_symlink():
                raise ValueError('已有同名配置但不属于本程序，未覆盖：' + str(target))
            return target
    raise ValueError('未发现 http 区块中的站点 include 目录，请按反向代理文档手动配置')


def ipv6():
    try:
        with socket.socket(socket.AF_INET6) as sock:
            sock.bind(('::1', 0))
        return True
    except OSError:
        return False


def cert_name(domain):
    return 'yuji-' + hashlib.sha256(domain.encode()).hexdigest()[:20]


def cert_paths(domain):
    base = CONF / 'letsencrypt' / 'live' / cert_name(domain)
    return base / 'fullchain.pem', base / 'privkey.pem'


def render(domain, port, tls, has_ipv6):
    domain = domain_name(domain)
    if type(port) is not int or not 1024 <= port <= 65535:
        raise ValueError('面板端口无效')
    challenge = ('    location ^~ /.well-known/acme-challenge/ {\n'
                 '        root ' + str(safe_nginx_path(WEBROOT)) + ';\n'
                 '        default_type text/plain;\n        autoindex off;\n'
                 '        limit_except GET { deny all; }\n'
                 '        add_header Cache-Control "no-store" always;\n'
                 '        try_files $uri =404;\n    }\n')
    text = MARKER
    if tls:
        text += 'map $http_upgrade $yuji_probe_autoproxy_connection { default upgrade; "" close; }\n'
    text += 'server {\n    listen 80;\n'
    if has_ipv6:
        text += '    listen [::]:80;\n'
    text += '    server_name ' + domain + ';\n    server_tokens off;\n    access_log off;\n' + challenge
    text += ('    location / { return 301 https://' + domain + '$request_uri; }\n'
             if tls else '    location / { return 503; }\n')
    text += '}\n'
    if not tls:
        return text
    cert, key = cert_paths(domain)
    text += 'server {\n    listen 443 ssl;\n'
    if has_ipv6:
        text += '    listen [::]:443 ssl;\n'
    text += '    server_name ' + domain + ';\n'
    text += '    if ($host != ' + domain + ') { return 421; }\n'
    text += '    ssl_certificate ' + str(safe_nginx_path(cert)) + ';\n'
    text += '    ssl_certificate_key ' + str(safe_nginx_path(key)) + ';\n'
    text += """    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_session_timeout 1d;
    ssl_session_tickets off;
    server_tokens off;
    autoindex off;
    access_log off;
    client_max_body_size 24m;
    client_header_timeout 10s;
    client_body_timeout 60s;
    send_timeout 60s;
    keepalive_timeout 30s;
    real_ip_header CF-Connecting-IP;
"""
    text += ''.join('    set_real_ip_from ' + item + ';\n' for item in CF_RANGES)
    text += challenge + """    location = /_internal/health { return 404; }
    location / {
        proxy_pass http://127.0.0.1:""" + str(port) + """;
        proxy_http_version 1.1;
        proxy_set_header Host """ + domain + """;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header X-Probe-Gateway "";
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $yuji_probe_autoproxy_connection;
        proxy_buffering off;
        proxy_request_buffering off;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }
}
"""
    return text


class SameHostRedirect(urllib.request.HTTPRedirectHandler):
    max_redirections = 5

    def __init__(self, domain):
        self.domain = domain

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        value = urllib.parse.urlsplit(newurl)
        if (value.hostname != self.domain or value.scheme not in ('http', 'https')
                or value.port not in (None, 80, 443) or value.username or value.password):
            raise ValueError('域名被转向其他站点')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def public_get(domain, path, https=False):
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}), SameHostRedirect(domain))
    request = urllib.request.Request(('https://' if https else 'http://') + domain + path,
                                     headers={'User-Agent': 'Yuji-Probe-HTTPS/1.0', 'Cache-Control': 'no-cache'})
    with client.open(request, timeout=10) as response:
        return response.status, response.read(65536)


def http_preflight(domain):
    token = secrets.token_urlsafe(32)
    path = WEBROOT / '.well-known' / 'acme-challenge' / token
    write(path, token, 0o644)
    try:
        # A graceful Nginx reload is asynchronous: first wait for our local ACME route.
        for attempt in range(15):
            try:
                connection = http.client.HTTPConnection('127.0.0.1', 80, timeout=2)
                connection.request('GET', '/.well-known/acme-challenge/' + token, headers={'Host': domain})
                response = connection.getresponse()
                ready = response.status == 200 and response.read(256) == token.encode()
                connection.close()
                if ready:
                    break
            except OSError:
                pass
            time.sleep(0.2)
        status, body = public_get(domain, '/.well-known/acme-challenge/' + token)
        return status == 200 and body == token.encode()
    except (OSError, ValueError, urllib.error.URLError):
        return False
    finally:
        path.unlink(missing_ok=True)


def ensure_certbot():
    binary = VENV / 'bin' / 'certbot'
    marker = VENV / '.yuji-managed'
    ready = VENV / '.yuji-ready'
    if VENV.exists():
        parents_safe(VENV)
        if not marker.is_file() or trusted(marker).read_text() != 'yuji-certbot-v1\n':
            raise ValueError('证书工具目录已存在但不属于本程序，请先检查 ' + str(VENV))
    else:
        directory(VENV, 0o755)
        write(marker, 'yuji-certbot-v1\n')
    if binary.is_file() and ready.is_file() and trusted(ready).read_text() == 'pip>=26.2;certbot5\n':
        parents_safe(binary.parent)
        trusted(binary)
        run([binary, '--version'], stdout=subprocess.DEVNULL)
        return binary
    values = platform()
    executable = '/usr/bin/python3'
    if values.get('ID') in ('debian', 'ubuntu'):
        apt_install('python3-venv')
    else:
        if values.get('VERSION_ID', '').split('.')[0] == '9' and values.get('ID') != 'fedora':
            run(['dnf', 'install', '-y', 'python3.11', 'python3.11-pip'], timeout=600)
            executable = '/usr/bin/python3.11'
        else:
            run(['dnf', 'install', '-y', 'python3', 'python3-pip'], timeout=600)
    print('\n  正在准备独立的 Certbot 证书工具…')
    run([executable, '-m', 'venv', VENV])
    env = {key: value for key, value in os.environ.items() if not key.startswith(('PIP_', 'PYTHON'))}
    env['PIP_CONFIG_FILE'] = '/dev/null'
    run([VENV / 'bin' / 'python', '-m', 'pip', 'install', '--upgrade', '--disable-pip-version-check',
         '--index-url', 'https://pypi.org/simple', '--only-binary=:all:', 'pip>=26.2'],
        env=env, timeout=300)
    run([VENV / 'bin' / 'python', '-m', 'pip', 'install', '--upgrade', '--disable-pip-version-check',
         '--index-url', 'https://pypi.org/simple', '--only-binary=:all:',
         'certbot>=5.8,<6', 'certbot-dns-cloudflare>=5.8,<6'], env=env, timeout=600)
    for folder, _, files in os.walk(VENV):
        os.chmod(folder, 0o755)
        for name in files:
            path = pathlib.Path(folder) / name
            if not path.is_symlink():
                path.chmod(0o755 if os.access(path, os.X_OK) else 0o644)
    write(ready, 'pip>=26.2;certbot5\n')
    return binary


def certbot_args(binary):
    return [str(binary), '--config-dir', str(CONF / 'letsencrypt'),
            '--work-dir', str(WORK / 'certbot'), '--logs-dir', str(WORK / 'logs'),
            '--server', ACME_SERVER, '--non-interactive']


def certbot_run(args):
    log = WORK / 'last-certbot.log'
    if log.exists() or log.is_symlink():
        trusted(log)
    with log.open('wb') as stream:
        os.fchmod(stream.fileno(), 0o600)
        result = subprocess.run([str(value) for value in args], stdout=stream,
                                stderr=subprocess.STDOUT, timeout=300,
                                env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C.UTF-8',
                                     'CLOUDFLARE_BASE_URL': 'https://api.cloudflare.com/client/v4'})
    if result.returncode:
        raise ValueError('证书操作未完成，详情仅保存在 ' + str(log))


def cloudflare_credentials(domain):
    target = CONF / (cert_name(domain) + '-cloudflare.ini')
    if target.exists():
        trusted(target)
        if target.stat().st_mode & 0o077:
            raise ValueError('Cloudflare 凭据必须仅 root 可读')
        return target
    print('\n  HTTP 验证未通过。若域名使用 Cloudflare，可保持橙云并改用 DNS 验证。')
    print('  Token 只需此域名所属 Zone 的 DNS 编辑、Zone 读取权限。')
    print('  https://dash.cloudflare.com/profile/api-tokens')
    secret = getpass.getpass('  Cloudflare API Token（隐藏输入，留空退出）：').strip()
    if not re.fullmatch(r'[A-Za-z0-9_-]{20,256}', secret):
        raise ValueError('未提供有效 Token。请放行 ACME 路径后重试，或使用手动反向代理文档')
    write(target, 'dns_cloudflare_api_token = ' + secret + '\n')
    return target


def issue(domain, binary, previous_method=None):
    base = certbot_args(binary) + ['certonly', '--agree-tos', '--register-unsafely-without-email',
                                  '--key-type', 'ecdsa', '--elliptic-curve', 'secp256r1',
                                  '--cert-name', cert_name(domain), '-d', domain, '--keep-until-expiring']
    if previous_method != 'cloudflare' and http_preflight(domain):
        try:
            certbot_run(base + ['--webroot', '-w', str(WEBROOT), '--preferred-challenges', 'http'])
            return 'http'
        except ValueError:
            print('  HTTP 证书验证失败，可改用 Cloudflare DNS 验证。')
    credentials = cloudflare_credentials(domain)
    certbot_run(base + ['--dns-cloudflare', '--dns-cloudflare-credentials', str(credentials),
                       '--dns-cloudflare-propagation-seconds', '60'])
    return 'cloudflare'


def certificate_valid(domain, seconds=3600):
    cert, key = cert_paths(domain)
    if not cert.is_file() or not key.is_file():
        return False
    base = (CONF / 'letsencrypt').resolve()
    for path in (cert, key):
        resolved_path = path.resolve()
        if base not in resolved_path.parents:
            raise ValueError('证书链接指向托管目录之外')
        trusted(resolved_path)
    if key.stat().st_mode & 0o077:
        raise ValueError('证书私钥权限过宽')
    # OpenSSL treats these as separate actions; combining them only checks the last.
    for arguments in [['-checkhost', domain], ['-checkend', str(seconds)]]:
        result = subprocess.run(['openssl', 'x509', '-in', str(cert), '-noout', *arguments],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        if result.returncode:
            return False
    return True


def local_https(domain):
    with socket.create_connection(('127.0.0.1', 443), timeout=8) as raw:
        with ssl.create_default_context().wrap_socket(raw, server_hostname=domain) as conn:
            conn.sendall(('GET / HTTP/1.1\r\nHost: ' + domain + '\r\nConnection: close\r\n\r\n').encode())
            response = http.client.HTTPResponse(conn)
            response.begin()
            if response.status != 200:
                raise ValueError('本机 HTTPS 反向代理检查失败：HTTP ' + str(response.status))


class Transaction:
    def __init__(self):
        self.files = {}

    def put(self, path, data, mode=0o600):
        if path not in self.files:
            if path.exists() or path.is_symlink():
                trusted(path)
                self.files[path] = (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
            else:
                self.files[path] = None
        write(path, data, mode)

    def rollback(self):
        errors = []
        for path, previous in reversed(list(self.files.items())):
            try:
                if previous is None:
                    path.unlink(missing_ok=True)
                else:
                    write(path, previous[0], previous[1])
            except OSError as error:
                errors.append(str(path) + ': ' + str(error))
        if errors:
            raise RuntimeError('文件恢复未完成：' + '；'.join(errors))


def install_renewal(transaction):
    service = """[Unit]
Description=Yuji Probe HTTPS certificate renewal
Wants=network-online.target
After=network-online.target
[Service]
Type=oneshot
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=full
ReadWritePaths=__HTTPS_CONF__
ExecStart=/usr/bin/python3 -I __ROOT__/ops/nginx_setup.py --renew
TimeoutStartSec=600
"""
    service = service.replace("__HTTPS_CONF__", str(CONF)).replace("__ROOT__", str(ROOT))
    timer = """[Unit]
Description=Check Yuji Probe HTTPS certificate twice daily
[Timer]
OnCalendar=*-*-* 00,12:00:00
RandomizedDelaySec=1800
Persistent=true
[Install]
WantedBy=timers.target
"""
    marker = '# Managed by Yuji Probe HTTPS.\n'
    for name, content in [(NAME + '-https-renew.service', service), (TIMER, timer)]:
        path = SYSTEMD / name
        if path.exists() and not trusted(path).read_text().startswith(marker):
            raise ValueError('已有同名续期服务但不属于本程序，未覆盖：' + str(path))
        transaction.put(path, marker + content, 0o644)
    run(['systemctl', 'daemon-reload'])
    run(['systemctl', 'enable', '--now', TIMER], stdout=subprocess.DEVNULL)


def prepare():
    directory(CONF.parent)
    directory(CONF)
    directory(WORK)
    directory(WEBROOT, 0o755)
    directory(WEBROOT / '.well-known', 0o755)
    directory(WEBROOT / '.well-known' / 'acme-challenge', 0o755)



def selinux_prepare(port):
    """Allow only the panel's TCP port, rather than enabling all outbound connections."""
    enforce = pathlib.Path('/sys/fs/selinux/enforce')
    if not enforce.exists():
        return
    run(['dnf', 'install', '-y', 'policycoreutils-python-utils', 'checkpolicy',
         'policycoreutils-devel'], timeout=600)
    label = 'yuji_probe_backend_port_t'
    existing = capture(['semanage', 'port', '-l'])
    assigned = False
    for line in existing.splitlines():
        fields = line.split(None, 2)
        if len(fields) != 3 or fields[1] != 'tcp':
            continue
        for item in fields[2].replace(' ', '').split(','):
            if not re.fullmatch(r'\d+(?:-\d+)?', item):
                continue
            bounds = [int(value) for value in item.split('-')]
            if bounds[0] <= port <= bounds[-1]:
                if fields[0] == label:
                    assigned = True
                elif fields[0] not in ('unreserved_port_t', 'ephemeral_port_t', 'port_t'):
                    raise ValueError('该端口已有其他 SELinux 用途，未重标记：' + fields[0])
    policy = WORK / 'yuji_probe_https.te'
    modules = capture(['semodule', '-l'])
    if re.search(r'^yuji_probe_https\s', modules, re.M) and not policy.exists():
        raise ValueError('已有同名 SELinux 模块，未覆盖')
    content = ('module yuji_probe_https 1.0;\n'
               'require { type httpd_t; attribute port_type; class tcp_socket name_connect; }\n'
               'type yuji_probe_backend_port_t, port_type;\n'
               'allow httpd_t yuji_probe_backend_port_t:tcp_socket name_connect;\n')
    reserved_write(policy, content, 0o600)
    run(['checkmodule', '-M', '-m', '-o', WORK / 'yuji_probe_https.mod', policy])
    run(['semodule_package', '-o', WORK / 'yuji_probe_https.pp',
         '-m', WORK / 'yuji_probe_https.mod'])
    run(['semodule', '-i', WORK / 'yuji_probe_https.pp'])
    if not assigned:
        run(['semanage', 'port', '-a', '-t', label, '-p', 'tcp', str(port)])
    custom = capture(['semanage', 'fcontext', '-l', '-C'])
    for pattern, kind in [(str(CONF) + '(/.*)?', 'cert_t'),
                          (str(WEBROOT) + '(/.*)?', 'httpd_sys_content_t')]:
        matching = [line for line in custom.splitlines() if line.startswith(pattern + ' ')]
        if matching:
            if not all(':' + kind + ':' in line for line in matching):
                raise ValueError('已有不同的 SELinux 文件标签，未覆盖：' + pattern)
        else:
            run(['semanage', 'fcontext', '-a', '-t', kind, pattern])
    run(['restorecon', '-RF', CONF, WEBROOT])

def setup(settings, configure, state):
    print('\n  配置 Nginx + HTTPS\n  ──────────────────────────────')
    print('  请先将域名解析到本机；Cloudflare 橙云可以保持开启。')
    print('  80 / 443 需可达；Cloudflare 回源模式使用 Full (strict)。')
    print('  将申请 Let’s Encrypt 证书并自动续期；填写域名即同意其订阅协议。')
    print('  https://letsencrypt.org/repository/\n')
    domain = domain_name(input('  域名：'))
    origin = 'https://' + domain
    old_origin, port = settings['origin'], settings['port']
    if type(port) is not int or not 1024 <= port <= 65535:
        raise ValueError('现有面板端口无效')
    if origin != old_origin:
        if json.loads((state / 'nodes.json').read_text()).get('nodes', []):
            raise ValueError('已有节点时请先通过后台迁移同步新地址，避免 Agent 失联')
        if old_origin.startswith('https://'):
            raise ValueError('已有 HTTPS 入口，请先按面板迁移流程更改访问地址后再配置对应域名')
    resolved(domain)
    prepare()
    previous = read_json(CONF / 'site.json') if (CONF / 'site.json').exists() else None
    if previous and previous.get('domain') != domain:
        raise ValueError('已有其他自动托管域名，请先按迁移文档处理，未覆盖旧站点')
    nginx = discover_nginx()
    busy_ports()
    site = choose_site(nginx, domain, previous)
    binary = ensure_certbot()
    selinux_prepare(port)
    has_ipv6 = ipv6()
    transaction, changed_origin = Transaction(), False
    timer_active = subprocess.run(['systemctl', 'is-enabled', '--quiet', TIMER],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
    try:
        if not previous or not certificate_valid(domain):
            transaction.put(site, render(domain, port, False, has_ipv6), 0o644)
            nginx_reload(nginx)
        if previous and certificate_valid(domain, 30 * 86400):
            method = previous['method']
        else:
            print('\n  正在验证域名并申请证书…')
            method = issue(domain, binary, previous.get('method') if previous else None)
        if pathlib.Path('/sys/fs/selinux/enforce').exists():
            run(['restorecon', '-RF', CONF, WEBROOT])
        if not certificate_valid(domain):
            raise ValueError('证书、域名或有效期校验未通过')
        content = render(domain, port, True, has_ipv6)
        transaction.put(site, content, 0o644)
        nginx_reload(nginx)
        if settings.get('listen') != '127.0.0.1:' + str(port) or old_origin != origin:
            if origin != old_origin and json.loads((state / 'nodes.json').read_text()).get('nodes', []):
                raise ValueError('配置期间新增了节点，请先迁移 Agent 地址后重试')
            configure(origin, port)
            changed_origin = True
        for attempt in range(15):
            try:
                local_https(domain)
                break
            except (OSError, ValueError, ssl.SSLError):
                if attempt == 14:
                    raise
                time.sleep(0.2)
        nginx = discover_nginx(False)
        data = {'domain': domain, 'site': str(site), 'nginx': nginx, 'port': port,
                'method': method, 'site_sha256': hashlib.sha256(content.encode()).hexdigest()}
        transaction.put(CONF / 'site.json', json.dumps(data, ensure_ascii=False) + '\n')
        install_renewal(transaction)
    except BaseException as original:
        errors = []
        tasks = []
        if not timer_active:
            tasks.append(lambda: subprocess.run(['systemctl', 'disable', '--now', TIMER],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30))
        tasks += [transaction.rollback, lambda: run(['systemctl', 'daemon-reload']),
                  lambda: nginx_reload(nginx)]
        if changed_origin:
            tasks.append(lambda: configure(old_origin, port))
        for task in tasks:
            try:
                task()
            except Exception as error:
                errors.append(str(error))
        if errors:
            raise RuntimeError('配置未完成：' + str(original) + '；部分恢复操作失败：'
                               + '；'.join(errors)) from original
        raise
    print('\n  已完成  ' + origin)
    print('  站点配置  ' + str(site))
    print('  证书续期  每日检查两次 · ' + ('Cloudflare DNS' if method == 'cloudflare' else 'HTTP'))
    print('  主控监听  127.0.0.1:' + str(port))
    try:
        status, _ = public_get(domain, '/', True)
        if status != 200:
            raise ValueError('unexpected status')
        print('  公网 HTTPS 检查通过。')
    except (OSError, ValueError, urllib.error.URLError):
        print('  本机 HTTPS 已通过。公网检查未通过，请检查安全组、Cloudflare Full (strict) 与 WAF。')
    print('  使用说明：https://github.com/YJVIst/YJmonitor/blob/main/docs/自动HTTPS.md')


def unconfigure():
    """Remove only our unchanged vhost and renewal units during explicit uninstall."""
    record = CONF / 'site.json'
    if not record.exists():
        return
    data = read_json(record)
    nginx = discover_nginx(False)
    site = choose_site(nginx, domain_name(data['domain']), data)
    units = [SYSTEMD / TIMER, SYSTEMD / NAME + '-https-renew.service']
    for path in units:
        if path.exists() and not trusted(path).read_text().startswith('# Managed by Yuji Probe HTTPS.\n'):
            raise ValueError('续期服务已被手动替换，未删除：' + str(path))
    content, mode = site.read_bytes(), stat.S_IMODE(site.stat().st_mode)
    site.unlink()
    try:
        nginx_check(nginx)
        if masters():
            nginx_reload(nginx)
    except BaseException:
        write(site, content, mode)
        if masters():
            nginx_reload(nginx)
        raise
    run(['systemctl', 'disable', '--now', TIMER], stdout=subprocess.DEVNULL)
    for path in units:
        path.unlink(missing_ok=True)
    record.unlink()
    run(['systemctl', 'daemon-reload'])


def renew():
    data = read_json(CONF / 'site.json')
    domain = domain_name(data['domain'])
    nginx = discover_nginx(False)
    if nginx != data['nginx']:
        raise ValueError('Nginx 实例或启动参数已变化，请重新运行 HTTPS 菜单核对')
    choose_site(nginx, domain, data)
    binary = VENV / 'bin' / 'certbot'
    trusted(binary)
    parents_safe(binary.parent)
    cert, _ = cert_paths(domain)
    before = hashlib.sha256(cert.read_bytes()).digest() if cert.is_file() else None
    certbot_run(certbot_args(binary) + ['renew', '--cert-name', cert_name(domain),
                                      '--no-random-sleep-on-renew', '--quiet'])
    if pathlib.Path('/sys/fs/selinux/enforce').exists():
        run(['restorecon', '-RF', CONF, WEBROOT])
    if not certificate_valid(domain):
        raise ValueError('续期后证书检查失败，未重载 Nginx')
    if hashlib.sha256(cert.read_bytes()).digest() != before:
        nginx_reload(nginx)
    else:
        print('证书仍有效，无需续期或重载 Nginx。')


if __name__ == '__main__':
    try:
        if os.geteuid() != 0 or sys.argv[1:] != ['--renew']:
            raise ValueError('请使用 sudo YJ https；本入口仅供证书续期')
        import fcntl
        os.umask(0o077)
        with open('/run/' + NAME + '-manage.lock', 'a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            renew()
    except (Exception, KeyboardInterrupt) as exc:
        print('HTTPS 操作未完成：' + str(exc), file=sys.stderr)
        sys.exit(1)
