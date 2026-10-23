# The shell prototype

The same thing the app does, in bash. Useful when you want to drive it from a terminal or an
agent, and the fastest way to test a provisioning change without rebuilding.

```bash
./vpn-up                               # provision in us-east-1
VPN_REGION=eu-west-2 ./vpn-up          # ...or anywhere else
./vpn-check                            # IPv4, IPv6, DNS and first-hop leak checks
./vpn-down                             # destroy and release the IP
```

**These share state with the app.** Both use `burrow-<region>` for resources and
`~/.config/burrow/<region>.conf` for configs, so you can create a server in the GUI and destroy it
here, or the reverse. If you change the naming in one place, change it in
`src-tauri/src/node.rs` too, or a teardown will quietly miss a running server that keeps billing.

Needs `awscli` and `wireguard-tools`. The app needs neither, which is rather the point of the app.
