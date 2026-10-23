# Burrow

**Your own VPN, on your own cloud account.** No subscription, no provider in the middle, no logs
kept by anyone but you. Burrow creates a WireGuard server in your AWS account, hands you a config,
and destroys it again when you're done. You pay AWS directly — about **$5 a month** if you leave it
running, pennies if you don't.

You dig your own tunnel. Hence the name.

> **Status: early.** The app provisions and destroys servers and manages credentials. Raising the
> tunnel still needs one terminal command — see [Known gaps](#known-gaps).

## Why

Commercial VPNs ask you to trust a company with every packet you send, and their entire business
model depends on you believing a no-logs promise you cannot verify. The alternative has always
existed — run WireGuard on a cheap VM — but it meant an SSH session, a hand-written config, and
knowing what `iptables -t nat -A POSTROUTING` does.

Burrow is that alternative with the terminal taken out of it. The only party who can see your
traffic is you, because the server is yours.

## What it costs

One Lightsail `nano_3_0` instance: **$5/month, billed hourly, 1 TB of transfer included.**

**Lightsail, not EC2** — this is the decision the whole project rests on. EC2 charges about
$0.09/GB egress, and *all* your browsing leaves through that instance. 1 TB would be roughly $90.
On EC2 this would cost more than the subscription it replaces.

## Using it

1. Open Burrow. It walks you through creating an AWS account, a restricted IAM user, and an access
   key, with a link to each page.
2. Paste the access key. Burrow checks it against AWS immediately and tells you if the Lightsail
   permission is missing, rather than failing later.
3. Pick a location, click **Create server**. Takes about two minutes.
4. Connect, browse, and click **Destroy server** when you're done. Billing stops.

## Design

**No Burrow server.** The app talks straight to the AWS API from your machine. There is no backend,
no account to create, and nothing of yours on our infrastructure — because there isn't any. This
is also the only honest way to sell "your own VPN".

**Credentials go in the OS keychain**, never in a config file: macOS Keychain, Windows Credential
Manager, Secret Service on Linux.

**Keys are generated in-process.** Burrow uses `x25519-dalek` rather than shelling out to
`wg genkey`, so no private key is ever visible in the process table. The output is verified
byte-identical to `wg pubkey` by a unit test, because a mismatch there would break every tunnel
with no useful error.

**The server is never logged into.** Both keypairs are made locally and handed to the instance in
its launch script, so it configures itself on first boot. Port 22 is never opened; only UDP 51820.

*Trade-off, stated plainly:* the server's private key travels through the Lightsail API over TLS
and is readable in user-data on the instance. Fine for a single-user node you destroy on demand.
Multi-tenant would require generating on-box and fetching only the public key.

**Names are the state.** Resources are named from the region (`burrow-us-east-1`), so there's no
local state file to lose and a half-finished provision can always be cleaned up.

**IPv6 fails closed.** The client config carries both an IPv4 and an IPv6 address. Without the v6
address, macOS silently refuses the `::/1` routes, `wg-quick` reports it and carries on, and every
IPv6-capable site — Google, YouTube, Cloudflare — is reached over your real connection while IPv4
looks perfectly tunnelled. That bug shipped in an earlier prototype and is exactly why `vpn-check`
exists in `scripts/`.

## Known gaps

- **Connecting still needs one terminal command.** `wg-quick` requires root, and doing that
  properly means a privileged helper installed via `SMAppService`, not a password prompt on every
  connect. The app shows the exact command meanwhile.
- **No kill-switch.** If the tunnel drops, traffic silently returns to your ISP. This is the most
  important missing feature, ahead of more locations.
- **US regions only** so far.
- **Requires `wireguard-tools`** (`brew install wireguard-tools`). Bundling it would remove the
  last manual dependency.
- macOS only in practice, though nothing in the Rust is macOS-specific.

## Building

```bash
npm install
npm run tauri dev      # run it
npm run tauri build    # bundle it
cd src-tauri && cargo test --lib
```

Needs Rust and Node. No Xcode required.

## Repo layout

| | |
|---|---|
| `src/` | Frontend — plain HTML, CSS and JS, no framework |
| `src-tauri/src/creds.rs` | Keychain storage and credential validation |
| `src-tauri/src/keys.rs` | WireGuard keypairs, with the `wg pubkey` compatibility test |
| `src-tauri/src/node.rs` | Lightsail provisioning and teardown |
| `scripts/` | The original shell prototype — `vpn-up`, `vpn-down`, `vpn-check`. Still the quickest way to test a change against real AWS |

## Licence

MIT.
