# Working on Burrow with Claude Code

Burrow runs a WireGuard VPN exit node on the user's own AWS account. This file tells you how to
drive it, and the handful of things that have already gone wrong so they don't go wrong again.

**You can do the whole thing for someone.** If they say *"connect me to London"*, that means:
provision a server in `eu-west-2`, raise the tunnel, confirm their public address actually
changed, and tell them what it costs. Then destroy it when they're done.

---

## Before anything: credentials

The user needs an AWS access key with Lightsail permission. **Never ask them to paste a secret key
into the chat** — it ends up in the transcript. Have them run this themselves:

```
! aws configure
```

(The `!` prefix runs it in their session.) If they don't have a key yet, walk them through it:

1. Sign in at **https://console.aws.amazon.com/** — root user signs in with the account *email*,
   not a username.
2. **IAM → Users → Create user**, name it `burrow`.
3. **Attach policies directly** → tick `AmazonLightsailFullAccess`.
4. Open the user → **Security credentials** → **Create access key** → **Command Line Interface**.
5. The secret is shown **once**.

Never create access keys on the root user. Check it works before doing anything else:

```bash
aws sts get-caller-identity
aws lightsail get-regions --region us-east-1 --query 'regions[0].name'
```

`InvalidClientTokenId` means the key is wrong or revoked, not that the region is wrong.

They also need `wireguard-tools` (`brew install wireguard-tools`). The GUI app does not, but the
scripts do.

---

## Doing it

Everything is in `scripts/`. Resource names come from the region, so there is no state file.

```bash
VPN_REGION=eu-west-2 scripts/vpn-up      # provision; ~2 minutes
scripts/vpn-check                        # prove it: IPv4, IPv6, DNS, first hop
VPN_REGION=eu-west-2 scripts/vpn-down    # destroy; billing stops
```

| Ask | `VPN_REGION` |
|---|---|
| United States, east | `us-east-1` (Virginia), `us-east-2` (Ohio) |
| United States, west | `us-west-2` (Oregon) |
| Europe | `eu-central-1` (Frankfurt), `eu-west-2` (London), `eu-west-1` (Ireland) |
| Australia | `ap-southeast-2` (Sydney) |

Any other Lightsail region works too — the instance size and availability zone are discovered per
region, nothing is hardcoded.

### Raising the tunnel

`wg-quick` needs root. `sudo` will not work from a tool call: there is no TTY, and
`sudo: a terminal is required to read the password`. Use the system authorisation dialog instead —
the user types their password into a native prompt:

```bash
CONF="$HOME/.config/burrow/eu-west-2.conf"
osascript -e "do shell script \"export PATH=/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin; wg-quick up '$CONF'\" with administrator privileges"
```

Tell them to expect the dialog before you run it, or it looks like the app hung.

### Always verify, never assume

```bash
scripts/vpn-check
```

A changed IPv4 address is **not** proof the VPN works. Run the full check. See the IPv6 note below
for why.

---

## Things that have already gone wrong

**Lightsail prepends its own `#!/bin/sh` bootstrap to user-data** and runs the concatenation under
dash. A `#!/bin/bash` line in a launch script is an inert comment in the middle of someone else's
file, and the first bashism kills everything after it:

```
/var/lib/cloud/instance/scripts/part-001: 17: set: Illegal option -o pipefail
```

The instance reports `running` with nothing installed. Stay POSIX in user-data; write the real
script to disk and `exec bash` on it.

**IPv6 leaks even with `AllowedIPs = ::/0`.** macOS will not route v6 to an interface that has no
v6 address, so the `::/1` routes fail with *"Network is unreachable"*, `wg-quick` prints it and
carries on, and every IPv6-capable site — Google, YouTube, Cloudflare — is reached over the real
ISP connection. **IPv4 looks perfectly tunnelled the whole time.** Both ends carry ULA addresses
(`fd00:8::1/64` and `fd00:8::2/128`) to prevent this. Do not remove them.

**macOS TCC blocks root from reading `~/Desktop`, `~/Documents` and `~/Downloads`.** A config in
any of those gives `wg-quick: Operation not permitted`, which reads like a WireGuard fault and
isn't. Configs live in `~/.config/burrow/`.

**An unattached Lightsail static IP bills monthly, forever.** `vpn-down` detaches, releases, and
then re-lists instances *and* static IPs to prove nothing survived. If you ever write a teardown
path, do the same.

**Bundle ids are region-suffixed** — `nano_3_0` in the US and Europe, `nano_3_2` in Sydney — and
allowances differ (1024 GB vs 512 GB for the same $5). Discover the bundle; never hardcode it.

**`running` is not `ready`.** A VM can boot and report running with the install failed. `vpn-up`
polls for a marker the launch script writes last and prints the setup log if it never appears.

---

## Rules

**Always tear down.** A forgotten instance costs about $5 a month in silence. If you provisioned
something during a session, destroy it or say clearly that it is still running. When in doubt:

```bash
for r in $(aws lightsail get-regions --query 'regions[].name' --output text); do
  aws lightsail get-instances --region "$r" --query 'instances[].name' --output text
  aws lightsail get-static-ips --region "$r" --query 'staticIps[].name' --output text
done
```

**Never commit a secret.** This repo is public. Client configs hold a WireGuard private key and
live outside the tree in `~/.config/burrow/`; `.gitignore` has `*.conf` as a backstop. Scan before
pushing:

```bash
git diff --cached | grep -nE "AKIA[0-9A-Z]{16}|PrivateKey *= *[A-Za-z0-9+/]{43}="
```

**Keep the app and the scripts in step.** They are two front ends onto the same resources —
`burrow-<region>` and `~/.config/burrow/<region>.conf`. If you rename in
`src-tauri/src/node.rs`, rename in `scripts/lib/common.sh` too, or a teardown will miss a running
server.

**Say what it costs.** Roughly $5/month billed hourly. Mention it when provisioning and again if
you leave something running.

---

## Building the app

```bash
npm install
npm run tauri dev
cd src-tauri && cargo test --lib
```

Rust and Node; no Xcode. `src-tauri/src/` is `creds.rs` (keychain + validation), `keys.rs`
(WireGuard keypairs, with a test asserting they match `wg pubkey`), `node.rs` (Lightsail),
`tunnel.rs` (privileged connect).

---

## The globe

`src/world.js` is vendored Natural Earth 1:110m country geometry, public domain, regenerated with
`python3 tools/build-world.py`. It is committed rather than fetched because Burrow must work
offline, and an app that phones out for a map on launch is a poor look for something selling
privacy.

`src/globe.js` draws it on a canvas with an orthographic projection. Two things are load-bearing:

- **No trigonometry per point per frame.** Latitude and longitude never change, so
  `cos(lat)·sin(lon)`, `cos(lat)·cos(lon)` and `sin(lat)` are precomputed into flat arrays at
  load. Rotation is then the angle-addition identity — four multiplies per point. With ~10,000
  points at 60fps, calling `Math.sin` per point would not hold frame rate.
- **Points behind the horizon are projected onto the limb, not dropped.** Dropping them breaks
  the ring and tears the fill on any country spanning the edge — Russia and Antarctica worst.
  Pushing them to the rim keeps every ring closed and the circular clip hides the join.
