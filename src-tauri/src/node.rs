//! Creating and destroying the exit node on Lightsail.
//!
//! Resource names are derived from the region, so the names are the state: there is no local file
//! to lose, and a half-finished provision can always be cleaned up by a teardown.

use crate::creds;
use crate::keys;
use aws_sdk_lightsail::types::PortInfo;
use aws_sdk_lightsail::Client;
use serde::Serialize;

/// $5/mo with 1024 GB of transfer. Not nano_ipv6_3_0 at $3.50 — that one is IPv6-only and has no
/// public IPv4 address, so it cannot act as a general-purpose exit node.
const BUNDLE: &str = "nano_3_0";
const BLUEPRINT: &str = "ubuntu_24_04";
const PORT: i32 = 51820;

const SERVER_V4: &str = "10.8.0.1/24";
const SERVER_V6: &str = "fd00:8::1/64";
const CLIENT_V4: &str = "10.8.0.2/32";
const CLIENT_V6: &str = "fd00:8::2/128";

pub fn instance_name(region: &str) -> String {
    format!("burrow-{region}")
}
pub fn static_ip_name(region: &str) -> String {
    format!("burrow-ip-{region}")
}

#[derive(Serialize, Clone, Debug)]
pub struct Node {
    pub region: String,
    pub endpoint: String,
    pub state: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Provisioned {
    pub node: Node,
    /// A complete wg-quick config. Holds the client private key, so it is written straight to disk
    /// by the caller and never logged.
    pub config: String,
}

/// Lightsail PREPENDS its own `#!/bin/sh` bootstrap to user-data and runs the concatenation under
/// dash, so a `#!/bin/bash` line here would be an inert comment and the first bashism would abort
/// everything before WireGuard was installed. Stay POSIX out here; write the real script to disk
/// and hand it to bash explicitly.
fn launch_script(server_private: &str, client_public: &str) -> String {
    format!(
        r#"set -eu
cat >/root/burrow-setup.sh <<'SETUP'
#!/bin/bash
set -euxo pipefail
export DEBIAN_FRONTEND=noninteractive

# cloud-init races unattended-upgrades for the apt lock on first boot; retry rather than die.
for _ in $(seq 1 30); do
  if apt-get update -y && apt-get install -y wireguard; then break; fi
  sleep 10
done
command -v wg >/dev/null || {{ echo "FATAL: wireguard did not install"; exit 1; }}

echo 'net.ipv4.ip_forward=1' >/etc/sysctl.d/99-wireguard.conf
sysctl --system

# Interface names are not stable across images; read it rather than assume eth0, or the tunnel
# comes up and nothing is ever NATed out.
WAN="$(ip -4 route show default | awk '{{print $5; exit}}')"

umask 077
cat >/etc/wireguard/wg0.conf <<EOF
[Interface]
Address = {server_v4}, {server_v6}
ListenPort = {port}
PrivateKey = {server_private}
PostUp   = iptables -t nat -A POSTROUTING -o ${{WAN}} -j MASQUERADE; iptables -A FORWARD -i wg0 -j ACCEPT; iptables -A FORWARD -o wg0 -j ACCEPT
PostDown = iptables -t nat -D POSTROUTING -o ${{WAN}} -j MASQUERADE; iptables -D FORWARD -i wg0 -j ACCEPT; iptables -D FORWARD -o wg0 -j ACCEPT

[Peer]
PublicKey = {client_public}
AllowedIPs = {client_v4}, {client_v6}
EOF

systemctl enable --now wg-quick@wg0
touch /var/log/burrow-ready
SETUP
chmod +x /root/burrow-setup.sh
exec /bin/bash /root/burrow-setup.sh >>/var/log/burrow-setup.log 2>&1
"#,
        server_v4 = SERVER_V4,
        server_v6 = SERVER_V6,
        port = PORT,
        server_private = server_private,
        client_public = client_public,
        client_v4 = CLIENT_V4,
        client_v6 = CLIENT_V6,
    )
}

fn client_config(client_private: &str, server_public: &str, endpoint: &str, dns: &str) -> String {
    format!(
        "[Interface]\n\
         PrivateKey = {client_private}\n\
         Address = {CLIENT_V4}, {CLIENT_V6}\n\
         DNS = {dns}\n\
         \n\
         [Peer]\n\
         PublicKey = {server_public}\n\
         Endpoint = {endpoint}:{PORT}\n\
         # ::/0 with an IPv6 address on [Interface] above: without the address macOS refuses the\n\
         # ::/1 routes and every v6-capable site sees the real IP while v4 looks fine.\n\
         AllowedIPs = 0.0.0.0/0, ::/0\n\
         PersistentKeepalive = 25\n"
    )
}

pub async fn find(client: &Client, region: &str) -> Option<Node> {
    let name = instance_name(region);
    let out = client.get_instance().instance_name(&name).send().await.ok()?;
    let i = out.instance()?;
    Some(Node {
        region: region.to_string(),
        endpoint: i.public_ip_address().unwrap_or_default().to_string(),
        state: i
            .state()
            .and_then(|s| s.name())
            .unwrap_or("unknown")
            .to_string(),
    })
}

pub async fn provision(
    access_key_id: &str,
    secret: &str,
    region: &str,
    dns: &str,
) -> Result<Provisioned, String> {
    let config = creds::config_for(access_key_id, secret, region).await;
    let client = Client::new(&config);
    let iname = instance_name(region);
    let ipname = static_ip_name(region);

    if find(&client, region).await.is_some() {
        return Err(format!(
            "A Burrow node already exists in {region}. Disconnect and remove it first."
        ));
    }

    let server = keys::generate();
    let client_keys = keys::generate();

    // The static IP is allocated first: a quota or permission failure then costs nothing, and the
    // endpoint is known before the instance exists.
    if client
        .get_static_ip()
        .static_ip_name(&ipname)
        .send()
        .await
        .is_err()
    {
        client
            .allocate_static_ip()
            .static_ip_name(&ipname)
            .send()
            .await
            .map_err(|e| format!("could not allocate an IP address: {e}"))?;
    }
    let endpoint = client
        .get_static_ip()
        .static_ip_name(&ipname)
        .send()
        .await
        .ok()
        .and_then(|o| o.static_ip().and_then(|s| s.ip_address().map(String::from)))
        .ok_or("allocated an IP address but could not read it back")?;

    let zone = format!("{region}a");
    client
        .create_instances()
        .instance_names(&iname)
        .availability_zone(&zone)
        .blueprint_id(BLUEPRINT)
        .bundle_id(BUNDLE)
        .user_data(launch_script(&server.private, &client_keys.public))
        .send()
        .await
        .map_err(|e| format!("could not create the server: {e}"))?;

    // Poll rather than assume: attach-static-ip fails on an instance that is still pending.
    let mut running = false;
    for _ in 0..60 {
        if let Some(n) = find(&client, region).await {
            if n.state == "running" {
                running = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    if !running {
        return Err("the server never started. Remove it and try again.".into());
    }

    client
        .attach_static_ip()
        .static_ip_name(&ipname)
        .instance_name(&iname)
        .send()
        .await
        .map_err(|e| format!("could not attach the IP address: {e}"))?;

    // REPLACES the port set: Lightsail opens 22 and 80 by default and we want neither. WireGuard
    // never answers unauthenticated packets, so an open UDP port is not the exposure SSH would be.
    client
        .put_instance_public_ports()
        .instance_name(&iname)
        .port_infos(
            PortInfo::builder()
                .from_port(PORT)
                .to_port(PORT)
                .protocol(aws_sdk_lightsail::types::NetworkProtocol::Udp)
                .cidrs("0.0.0.0/0")
                .build(),
        )
        .send()
        .await
        .map_err(|e| format!("could not set the firewall: {e}"))?;

    Ok(Provisioned {
        node: Node {
            region: region.to_string(),
            endpoint: endpoint.clone(),
            state: "running".into(),
        },
        config: client_config(&client_keys.private, &server.public, &endpoint, dns),
    })
}

/// Delete the instance and release the IP. Both are attempted even if the first fails: an orphaned
/// static IP bills every month, so partial cleanup is worse than none.
pub async fn destroy(access_key_id: &str, secret: &str, region: &str) -> Result<Vec<String>, String> {
    let config = creds::config_for(access_key_id, secret, region).await;
    let client = Client::new(&config);
    let iname = instance_name(region);
    let ipname = static_ip_name(region);
    let mut done = Vec::new();
    let mut failed = Vec::new();

    if find(&client, region).await.is_some() {
        match client.delete_instance().instance_name(&iname).send().await {
            Ok(_) => done.push("server deleted".to_string()),
            Err(e) => failed.push(format!("could not delete the server: {e}")),
        }
    } else {
        done.push("no server to delete".to_string());
    }

    if client.get_static_ip().static_ip_name(&ipname).send().await.is_ok() {
        let _ = client.detach_static_ip().static_ip_name(&ipname).send().await;
        match client.release_static_ip().static_ip_name(&ipname).send().await {
            Ok(_) => done.push("IP address released".to_string()),
            Err(e) => failed.push(format!(
                "could not release the IP address — THIS KEEPS BILLING: {e}"
            )),
        }
    } else {
        done.push("no IP address to release".to_string());
    }

    if failed.is_empty() {
        Ok(done)
    } else {
        Err(failed.join("; "))
    }
}
