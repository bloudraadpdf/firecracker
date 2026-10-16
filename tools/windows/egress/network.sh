#!/usr/bin/env bash
# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
mode=${1:?Expected setup or cleanup}
state=${2:?Set a new absolute workspace state directory}
[[ "$state" = /* && $(id -u) = 0 ]]
remove() {
    local flag=$1
    shift
    if [[ -e "$state/$flag" ]]; then
        if "$@"; then rm "$state/$flag"; else result=1; fi
    fi
}
cleanup() {
    local result=0
    if [[ -e "$state/namespace" ]]; then test -z "$(ip netns pids "$prefix")" || return 1; fi
    remove nat iptables -t nat -D POSTROUTING -s 172.30.254.2/32 -o "$uplink" -m comment --comment "$comment" -j MASQUERADE
    remove inbound iptables -D FORWARD -o "${prefix}host" -d 172.30.254.2/32 -m conntrack --ctstate RELATED,ESTABLISHED -m comment --comment "$comment" -j ACCEPT
    remove outbound iptables -D FORWARD -i "${prefix}host" -s 172.30.254.2/32 -m comment --comment "$comment" -j ACCEPT
    remove route ip route del 172.30.254.0/30 via 172.30.252.2 dev "${prefix}host"
    remove link ip link del "${prefix}host"
    remove namespace ip netns del "$prefix"
    if [[ "$result" = 0 ]]; then
        rm "$state/config.json"
        rmdir "$state" || result=1
    fi
    return "$result"
}
if [[ "$mode" = cleanup ]]; then
    prefix=$(jq -er .prefix "$state/config.json")
    uplink=$(jq -er .uplink "$state/config.json")
    [[ "$prefix" =~ ^[a-z][a-z0-9]{0,9}$ ]]
    comment=firecracker-egress:$prefix
    cleanup
    exit
fi
test "$mode" = setup
prefix=${WINDOWS_NETWORK_PREFIX:-fce}
uplink=${WINDOWS_NETWORK_UPLINK:?Set the host uplink interface}
uid=${WINDOWS_NETWORK_UID:?Set the VMM user numeric UID}
[[ "$prefix" =~ ^[a-z][a-z0-9]{0,9}$ && "$uid" =~ ^[0-9]+$ ]]
comment=firecracker-egress:$prefix
test "$(sysctl -n net.ipv4.ip_forward)" = 1
ip link show "$uplink" > /dev/null
if ip netns list | awk '{print $1}' | grep -Fxq "$prefix"; then exit 1; fi
if ip link show "${prefix}host" > /dev/null 2>&1; then exit 1; fi
if ip link show "${prefix}up" > /dev/null 2>&1; then exit 1; fi
test -z "$(ip route show exact 172.30.254.0/30)"
test -z "$(ip route show exact 172.30.252.0/30)"
umask 077
mkdir "$state"
jq -nc --arg prefix "$prefix" --arg uplink "$uplink" '{prefix:$prefix,uplink:$uplink}' > "$state/config.json"
failed_setup() {
    local code=$?
    trap - EXIT
    cleanup || true
    exit "$code"
}
trap failed_setup EXIT
ip netns add "$prefix"
touch "$state/namespace"
ip link add "${prefix}host" type veth peer name "${prefix}up"
touch "$state/link"
ip link set "${prefix}up" netns "$prefix"
ip address add 172.30.252.1/30 dev "${prefix}host"
ip link set "${prefix}host" up
ip -n "$prefix" link set lo up
ip -n "$prefix" address add 172.30.252.2/30 dev "${prefix}up"
ip -n "$prefix" link set "${prefix}up" up
ip -n "$prefix" route add default via 172.30.252.1
ip netns exec "$prefix" ip tuntap add dev "${prefix}tap" mode tap user "$uid"
ip -n "$prefix" address add 172.30.254.1/30 dev "${prefix}tap"
ip -n "$prefix" link set "${prefix}tap" up
ip netns exec "$prefix" sysctl -w net.ipv4.ip_forward=1
ip route add 172.30.254.0/30 via 172.30.252.2 dev "${prefix}host"
touch "$state/route"
iptables -I FORWARD 1 -i "${prefix}host" -s 172.30.254.2/32 -m comment --comment "$comment" -j ACCEPT
touch "$state/outbound"
iptables -I FORWARD 1 -o "${prefix}host" -d 172.30.254.2/32 -m conntrack --ctstate RELATED,ESTABLISHED -m comment --comment "$comment" -j ACCEPT
touch "$state/inbound"
iptables -t nat -A POSTROUTING -s 172.30.254.2/32 -o "$uplink" -m comment --comment "$comment" -j MASQUERADE
touch "$state/nat"
trap - EXIT
printf 'namespace=%s tap=%stap guest=172.30.254.2/30 gateway=172.30.254.1\n' "$prefix" "$prefix"
