#!/usr/bin/env bash
# Shared configuration and helpers for vpn-up / vpn-down.
# Sourced, never run directly.

# Everything is named deterministically from REGION, so teardown needs no state file: the names ARE
# the state. A half-finished vpn-up can always be cleaned up by vpn-down, which is the property that
# matters most when the thing being leaked costs money every month.
REGION="${VPN_REGION:-us-east-1}"
BUNDLE="${VPN_BUNDLE:-nano_3_0}"        # $5/mo, 1024 GB transfer. nano_ipv6_3_0 is cheaper but has
                                        # no public IPv4, so it cannot serve as a general exit node.
BLUEPRINT="${VPN_BLUEPRINT:-ubuntu_24_04}"
ZONE="${VPN_ZONE:-${REGION}a}"
PORT="${VPN_PORT:-51820}"

INSTANCE="vpn-byoc-${REGION}"
STATIC_IP="vpn-byoc-ip-${REGION}"

# Tunnel addressing. /24 is far more than one peer needs, but leaves room for more without
# renumbering the server.
#
# The IPv6 ULA addresses are NOT decoration and must not be dropped. Without an IPv6 address on the
# interface, macOS refuses to install the ::/1 and 8000::/1 routes that AllowedIPs = ::/0 asks for
# ("route: writing to routing socket: Network is unreachable"), wg-quick prints the failure and
# carries on, and every IPv6-capable destination — Google, YouTube, Cloudflare, most large sites —
# is then reached over the real ISP connection with the user's true address. IPv4 looks perfectly
# tunnelled the whole time, which is what makes this leak so easy to miss.
#
# The server has no IPv6 upstream, so v6 entering the tunnel is simply dropped: it fails closed.
SERVER_WG_ADDR="10.8.0.1/24"
SERVER_WG_ADDR6="fd00:8::1/64"
CLIENT_WG_ADDR="10.8.0.2/32"
CLIENT_WG_ADDR6="fd00:8::2/128"
CLIENT_DNS="${VPN_DNS:-1.1.1.1, 1.0.0.1}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Configs live in ~/.config, NOT in the repo, for two reasons. They hold the client private key, so
# keeping them outside the working tree means they cannot be committed by accident. And on macOS,
# ~/Desktop, ~/Documents and ~/Downloads are TCC-protected: a root shell raised by osascript or a
# privileged helper gets "Operation not permitted" reading a config there, which surfaces as a
# confusing wg-quick failure that has nothing to do with WireGuard.
CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/vpn-byoc"
CONF_FILE="$CONF_DIR/${REGION}.conf"

aws_ls() { aws lightsail --region "$REGION" "$@"; }

say()  { printf '  %s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
die()  { printf '\nERROR: %s\n' "$*" >&2; exit 1; }

preflight() {
  command -v aws >/dev/null || die "aws CLI not found. brew install awscli"
  command -v wg  >/dev/null || die "wg not found. brew install wireguard-tools"
  aws sts get-caller-identity >/dev/null 2>&1 \
    || die "AWS credentials are not working. Run: aws configure"
}

# Lightsail reports an instance as existing well before it can be addressed, so every caller needs
# to wait on an explicit state rather than assuming create-instances returning means ready.
instance_state() {
  aws_ls get-instance --instance-name "$INSTANCE" \
    --query 'instance.state.name' --output text 2>/dev/null || echo "absent"
}

static_ip_exists() {
  aws_ls get-static-ip --static-ip-name "$STATIC_IP" >/dev/null 2>&1
}
