#!/bin/sh
# Cuts the running `turk-binary watch` off from every host it is connected to,
# for a number of seconds, with pf. Nothing else on the machine is affected.
#
# Usage: sudo scripts/net_fault.sh drop|reset SECONDS
#   drop   packets vanish: the stream has to notice through its own timeouts
#   reset  the kernel answers with a TCP reset: the stream fails at once
#
# Remote addresses are not printed: they identify the provider.
set -eu

mode=${1:-}
seconds=${2:-}
case "$mode" in
drop) per_host=2 ;;
reset) per_host=1 ;;
*)
    echo "usage: sudo $0 drop|reset SECONDS" >&2
    exit 2
    ;;
esac
case "$seconds" in
'' | *[!0-9]*)
    echo "SECONDS must be a whole number" >&2
    exit 2
    ;;
esac
[ "$(id -u)" -eq 0 ] || {
    echo "pf needs root: run with sudo" >&2
    exit 1
}

pid=$(for p in $(pgrep -x turk-binary || true); do
    ps -o command= -p "$p" | grep -q ' watch' && echo "$p"
done | head -n 1)
[ -n "$pid" ] || {
    echo "no running 'turk-binary watch'" >&2
    exit 1
}
hosts=$(lsof -nP -a -p "$pid" -iTCP -sTCP:ESTABLISHED -Fn |
    sed -n 's/^n.*->\(.*\):[0-9]*$/\1/p' | tr -d '[]' | sort -u)
[ -n "$hosts" ] || {
    echo "process $pid has no established TCP connections" >&2
    exit 1
}
count=$(printf '%s\n' "$hosts" | wc -l | tr -d ' ')
table=$(printf '%s\n' "$hosts" | paste -sd, -)

anchor=com.apple/turk-binary-fault
token=""
restore() {
    pfctl -a "$anchor" -F rules 2>/dev/null || true
    [ -z "$token" ] || pfctl -X "$token" 2>/dev/null || true
    echo "$(date -u +%H:%M:%SZ) restored"
}
trap restore EXIT INT TERM

# Reset leaves inbound open: the ACK for the next segment the server sends
# is answered with a reset at once, instead of waiting for a keepalive.
if [ "$mode" = drop ]; then
    printf '%s\n' \
        "block drop out quick proto tcp from any to { $table }" \
        "block drop in quick proto tcp from { $table } to any"
else
    echo "block return out quick proto tcp from any to { $table }"
fi | pfctl -a "$anchor" -f - 2>/dev/null
token=$(pfctl -E 2>&1 | sed -n 's/^Token : //p')
if ! pfctl -sr 2>/dev/null | grep -q 'anchor "com.apple/\*"'; then
    pfctl -f /etc/pf.conf 2>/dev/null
fi
# pf expands the host list: one rule per host and direction.
expected=$((count * per_host))
rules=$(pfctl -a "$anchor" -sr 2>/dev/null | grep -c . || true)
[ "$rules" -eq "$expected" ] || {
    echo "pf loaded $rules of $expected fault rules" >&2
    exit 1
}
# Connections pf already tracks would bypass the new rule.
for host in $hosts; do
    pfctl -k 0.0.0.0/0 -k "$host" >/dev/null 2>&1 || true
done
echo "$(date -u +%H:%M:%SZ) $mode: $count host(s) of pid $pid cut for ${seconds}s"
sleep "$seconds"
