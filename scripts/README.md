# The shell prototype

These are the scripts the app grew out of. They do the same thing with `bash` and the AWS CLI, and
they remain the fastest way to test a provisioning change against real AWS without rebuilding the
app.

```bash
./vpn-up      # provision   (VPN_REGION=us-west-2 ./vpn-up for another region)
./vpn-check   # IPv4, IPv6, DNS and first-hop leak checks
./vpn-down    # destroy and release the IP
```

They need `awscli` and `wireguard-tools`; the app needs neither, which is the point of the app.

Configs are written to `~/.config/vpn-byoc/`, the app's to `~/.config/burrow/`, so the two can
coexist without fighting over the same file.
