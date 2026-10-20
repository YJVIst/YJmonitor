#!/usr/bin/env bash
set +x
set -Eeuo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 077
YUJI_ROOT=$(cd -- "$(dirname -- "$0")" && pwd -P)
case "$YUJI_ROOT" in /opt/YJ) YUJI_SERVICE=YJ.service;; /opt/yuji-probe) YUJI_SERVICE=yuji-probe.service;; *) printf '请使用已安装的 sudo YJ。\n';exit 1;; esac
readonly YUJI_ROOT YUJI_SERVICE
readonly YUJI_OPS="$YUJI_ROOT/ops/manage.py"
yuji_color='' yuji_dim='' yuji_reset=''
if [[ -t 1 && ${TERM:-dumb} != dumb && -z ${NO_COLOR:-} ]]; then
 yuji_color=$'\033[36m';yuji_dim=$'\033[2m';yuji_reset=$'\033[0m'
fi
if (( $# )); then exec python3 "$YUJI_OPS" "$@";fi
(( EUID == 0 )) || { printf '请使用 sudo YJ。\n';exit 1;}
while true; do
 printf '\n  %s羽迹探针%s  /  管理\n' "$yuji_color" "$yuji_reset"
 printf '  %s──────────────────────────────%s\n' "$yuji_dim" "$yuji_reset"
 python3 "$YUJI_OPS" summary
 yuji_columns=$(tput cols 2>/dev/null || printf 80)
 [[ "$yuji_columns" =~ ^[0-9]+$ ]] || yuji_columns=80
 if (( yuji_columns < 48 )); then
 printf '\n  维护\n    1  检查并更新\n    2  重启服务\n    3  启动服务\n    4  停止服务\n    5  查看日志\n    6  运行详情\n\n  配置与数据\n    7  设置访问地址\n    8  创建备份\n    9  重置二步验证\n   10  反向代理文档\n   11  回退上个版本\n   12  卸载程序\n   13  配置 Nginx + HTTPS\n\n    0  退出\n\n'
 else
 printf '\n  维护\n    1  检查并更新       2  重启服务\n    3  启动服务         4  停止服务\n    5  查看日志         6  运行详情\n\n  配置与数据\n    7  设置访问地址     8  创建备份\n    9  重置二步验证    10  反向代理文档\n   11  回退上个版本     12  卸载程序\n   13  配置 Nginx + HTTPS\n\n    0  退出\n\n'
 fi
 read -r -p '  选择：' yuji_choice || exit 0
 case "$yuji_choice" in
  0) exit 0;;
  1) python3 "$YUJI_OPS" update || true;;
  2) python3 "$YUJI_OPS" restart || true;;
  3) python3 "$YUJI_OPS" start || true;;
  4) python3 "$YUJI_OPS" stop || true;;
  5) journalctl -u "$YUJI_SERVICE" -n 80 --no-pager;;
  6) systemctl status "$YUJI_SERVICE" --no-pager || true;;
  7) python3 "$YUJI_OPS" address || true;;
  8) python3 "$YUJI_OPS" backup || true;;
  9) python3 "$YUJI_OPS" reset-mfa || true;;
  10) printf '\n  https://github.com/YJVIst/YJmonitor/blob/main/docs/反向代理.md\n';;
  11) python3 "$YUJI_OPS" rollback || true;;
  12) python3 "$YUJI_OPS" uninstall || true; [[ -f "$YUJI_OPS" ]] || exit 0;;
  13) python3 "$YUJI_OPS" https || true;;
  *) printf '\n  请输入菜单中的编号。\n';;
 esac
 read -r -p $'\n  按回车返回…' _ || exit 0
done
