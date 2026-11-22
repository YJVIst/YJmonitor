# BaoTa manual deployment: paste INSIDE the site's existing HTTPS server { }.
# Do not add an http/server block or duplicate the site's existing locations.
# Backend must match the Rust service: -listen 127.0.0.1:19282.
# Keep the site's domain, certificate and ACME challenge configuration.
# For this unified layout, leave the site's rewrite include file empty/comments only.

autoindex off;
client_max_body_size 24m;
client_header_timeout 10s;
client_body_timeout 10s;
send_timeout 15s;
keepalive_timeout 30s;
# 不将安装链接的查询参数写入访问日志；保留站点原有 error_log。
access_log off;

# BEGIN YUJI LOCATIONS
location = /_internal/health { return 404; }
location ~ ^/(?:app|bin|storage|backend|src|source|docs|ops|qa|vendor|node_modules)(?:/|$) { return 404; }
location ~ /\. { return 404; }
location ~ \.php(?:/|$) { return 404; }

location ~ ^/api/(?:agent|terminal)$ {
    proxy_pass http://127.0.0.1:19282;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Probe-Gateway "";
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 185s;
    proxy_send_timeout 30s;
    proxy_buffering off;
}
location = /api/enroll/claim {
    proxy_pass http://127.0.0.1:19282;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Probe-Gateway "";
    proxy_set_header Connection "";
    proxy_read_timeout 185s;
    proxy_send_timeout 30s;
    proxy_buffering off;
}
location ^~ /assets/ {
    gzip on;
    gzip_vary on;
    gzip_min_length 1024;
    gzip_types application/javascript text/javascript text/css image/svg+xml;
    proxy_pass http://127.0.0.1:19282;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Probe-Gateway "";
}
location / {
    proxy_pass http://127.0.0.1:19282;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Probe-Gateway "";
    proxy_set_header Connection "";
    proxy_read_timeout 65s;
    proxy_send_timeout 30s;
}
# END YUJI LOCATIONS
