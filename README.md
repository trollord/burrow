# Burrow

**Your own VPN, on your own cloud account.** No subscription, no provider in the middle, no logs
kept by anyone but you. Burrow starts a WireGuard server in your AWS account, connects you to it,
and destroys it when you're done. You pay AWS directly — roughly **$5 a month** if you leave it
running, pennies if you don't.

You dig your own tunnel. Hence the name.

```
your laptop  ──encrypted──▶  a server you own  ──▶  the internet
                             (destroy it whenever)
```

> **Status: early but usable.** Create a server, connect, verify your address changed, destroy it.
> See [Known gaps](#known-gaps) for what's missing.

## Why

Commercial VPNs ask you to trust a company with every packet you send, and the business model
rests on you believing a no-logs promise you cannot check. The alternative has always existed —
run WireGuard on a cheap VM — but it meant an SSH session, a hand-written config, and knowing what
`iptables -t nat -A POSTROUTING` does.

Burrow is that alternative with the terminal taken out. The only party who can see your traffic is
you, because the server is yours.

---

## Two ways to use it

### 1. Ask Claude Code

Clone the repo, open it in [Claude Code](https://claude.com/product/claude-code), and say what you
want:

> connect me to London

It provisions the server, raises the tunnel, confirms your address actually changed, and tells you
what it costs. When you're done: *"disconnect and destroy it"*.

The instructions it follows are in [`CLAUDE.md`](CLAUDE.md) — worth reading even if you never use
an agent, because it documents every failure mode this project hit.

### 2. The app

```bash
npm install && npm run tauri dev
```

Paste your AWS key, pick a location, watch it dig, connect. Live cost readout, and it verifies
you're genuinely tunnelled rather than showing a green dot that means nothing.

### Or the scripts

```bash
VPN_REGION=eu-west-2 scripts/vpn-up     # provision
scripts/vpn-check                       # prove it works
VPN_REGION=eu-west-2 scripts/vpn-down   # destroy
```

All three paths create the same resources and mix freely — make a server in the app, destroy it
from the terminal, whatever suits.

Either way you'll need an AWS key (below) and `brew install wireguard-tools`.

---

## Getting an AWS key

Burrow needs a key that can create Lightsail servers and nothing else.

1. Sign in at **console.aws.amazon.com** — the root user signs in with the account *email*, not a
   username.
2. **IAM → Users → Create user**, call it `burrow`.
3. **Attach policies directly** → tick `AmazonLightsailFullAccess`.
4. Open the user → **Security credentials** → **Create access key** → **Command Line Interface**.
5. Copy both values. **The secret is shown once.**

The app stores them in your system keychain. For the scripts, run `aws configure`. Never create
access keys on the root user.

## Locations

| | |
|---|---|
| United States | Virginia, Ohio, Oregon |
| Europe | Frankfurt, London, Ireland |
| Australia | Sydney |

Any of Lightsail's 19 regions works — instance size and availability zone are discovered per
region, so adding one is a line in a list.

## What it costs

About **$5 a month**, billed hourly, so an evening is a couple of cents. Burrow reads the real
price from AWS rather than hardcoding it, because it isn't uniform: **Sydney charges the same $5
for 512 GB of traffic where Virginia gives 1024 GB**, and the instance size is named differently
there.

**Lightsail, not EC2** — the decision the whole project rests on. EC2 bills around $0.09/GB
egress, and *all* your browsing leaves through that instance; 1 TB would be roughly $90. On EC2
this would cost more than the subscription it replaces.

---

## How it's built

**No Burrow server exists.** The app talks straight to the AWS API from your machine. There's no
backend, no account to create, nothing of yours on our infrastructure — because there isn't any.
It's also the only honest way to sell "your own VPN": we couldn't log your traffic if we wanted to.

**Credentials live in the OS keychain**, never in a config file. You can save **several named
keys** — "personal", "work" — and switch between them; the secret goes in the keychain while the
name, account and access key id go in a plain index. That split is deliberate: listing your keys
never touches the keychain, so opening the app does not throw an authorisation prompt at you.

**Sessions are recorded locally** — where, how long, and how much data moved. Byte counts come
from the tunnel interface's own counters via `netstat`, which needs no privileges; `wg show`
would have meant asking for a password to read a statistic.

**Keys are generated in-process** with `x25519-dalek` rather than shelling to `wg genkey`, so no
private key ever appears in the process table. A unit test asserts the public key matches what
`wg pubkey` derives — a mismatch would break every tunnel with no useful error.

**The server is never logged into.** Both keypairs are made locally and handed to the instance in
its launch script, so it configures itself on first boot. Port 22 is never opened; only UDP 51820.

> *Trade-off, stated plainly:* the server's private key travels through the Lightsail API over TLS
> and is readable in user-data on the box. Fine for a single-user node you destroy on demand.
> Multi-tenant would need keys generated on the server with only the public key fetched back.

**Names are the state.** Resources are named from the region (`burrow-eu-west-2`), so there's no
state file to lose and a half-finished provision can always be cleaned up.

**Teardown actually tears down.** It deletes the instance *and* releases the static IP — an
orphaned Lightsail IP bills monthly, forever — then re-lists both to prove nothing survived.

**Connecting asks for admin once, not every time.** `wg-quick` rewrites your routing table and
DNS, so it must run as root. Burrow installs a small root daemon — you approve once and never
again.

The design detail that matters: **the helper accepts no file path and no config text.** A
WireGuard config can carry `PostUp = <any command>`, which `wg-quick` runs as root, so a helper
that took a path would be a local privilege escalation — and so would the tempting shortcut of a
passwordless `sudo wg-quick` rule, since the configs live in a user-writable directory. Instead
the helper takes a region name and two validated keys, and writes its own config into a
root-owned directory with the `PostUp` lines fixed in its source. There is no input through which
a command can reach root. Its socket is chowned to the installing user at mode 0600.

**IPv6 fails closed.** Both ends carry ULA addresses. Without them macOS silently refuses the
`::/1` routes and every IPv6-capable site sees your real address while IPv4 looks perfectly
tunnelled. That bug shipped in an earlier prototype, which is why the next section exists.

---

## Verifying it

A changed IPv4 address is not proof. `scripts/vpn-check` tests five things:

| Check | Why it's there |
|---|---|
| IPv4 egress | matches the exit node |
| **IPv6 egress** | the leak that hides behind a perfectly working IPv4 |
| DNS | queries exit at the node, not your ISP |
| First hop | packets really enter the tunnel |
| Geolocation | what the internet reports back |

Two things a shell can't test: **WebRTC** can expose your address through the browser
(`browserleaks.com/webrtc`), and **browser geolocation** resolves your position from nearby WiFi
regardless of any VPN — only revoking the permission fixes that.

## Known gaps

- **No kill-switch.** If the tunnel drops, traffic silently returns to your ISP. The most
  important missing feature, ahead of more locations.
- **Requires `wireguard-tools`.** Bundling it would remove the last manual dependency.
- **macOS only.** See below.

## Windows and Linux

Not supported yet. The halves differ:

**Already portable** — the Tauri shell, the AWS SDK, credential storage (`keyring` has
Windows Credential Manager and Secret Service backends compiled in), key generation, the whole
frontend, and of course the Ubuntu server itself.

**Not portable** — everything in `src-tauri/src/tunnel.rs`. It shells to `wg-quick`, elevates with
`osascript`, and reads `ifconfig`. Windows has none of those: WireGuard there registers a tunnel
as a **service** via `wireguard.exe /installtunnelservice`, elevates through UAC, and reports
state through the service rather than an interface listing.

There is also one outright bug: `config_dir()` reads `HOME`, which Windows does not set
(`USERPROFILE`). On Windows that fails *after* a billable server has been created — the worst
place to fail. It needs fixing before anyone runs this there.

Linux is closer: `wg-quick` exists, `ifconfig` may not (`ip addr` instead), and elevation would
want `pkexec` rather than `osascript`.

The `scripts/` work anywhere bash, the AWS CLI and `wireguard-tools` do, which includes WSL.

## Layout

| | |
|---|---|
| `CLAUDE.md` | Driving this with an agent, and every failure mode hit so far |
| `src/` | Frontend — plain HTML, CSS and JS, no framework |
| `src/globe.js` | The globe: orthographic projection, coastlines, travelling light |
| `src/world.js` | Vendored Natural Earth 110m geometry (public domain) |
| `tools/build-world.py` | Regenerates `world.js` |
| `src-tauri/src/sessions.rs` | Session history and data accounting |
| `src-tauri/src/creds.rs` | Named profiles: keychain secrets, plaintext index |
| `src-tauri/src/paths.rs` | Where files live, and the JSON helpers |
| `src-tauri/src/keys.rs` | WireGuard keypairs + the `wg pubkey` compatibility test |
| `src-tauri/src/node.rs` | Lightsail provisioning and teardown |
| `src-tauri/src/tunnel.rs` | Connect/disconnect, and the "am I really tunnelled" check |
| `src-tauri/src/bin/burrow-helper.rs` | The root daemon, and the privilege boundary |
| `src-tauri/src/helper.rs` | Its client, and the one-time install |
| `scripts/` | The same thing in bash |

## Building

```bash
npm install
npm run tauri dev           # run it
npm run tauri build         # bundle it
cd src-tauri && cargo test --lib
```

Rust and Node. No Xcode required.

## Licence

MIT.
