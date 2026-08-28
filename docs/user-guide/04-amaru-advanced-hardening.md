---
id: amaru-advanced-hardening
title: "Running an Amaru Node on Mainnet: System Hardening"
sidebar_label: "System Hardening"
description: Secure a Cardano stake pool server.
---

An Amaru host needs the same baseline security as any Cardano relay :
- Dedicated user for the node process
- SSH key auth
- Firewall
- System updates
- Fail2ban

Please refer to the existing developers.cardano.org documentation : [Hardening the server](https://developers.cardano.org/docs/operators/deployment-scenarios/hardening-server/).

Here are some specific details regarding the implementation of Amaru :

## Dedicated user

The Debian/RPM package automatically creates the `amaru` system user : see [Running Amaru as a systemd service](05-amaru-advanced-installation.md#running-amaru-as-a-systemd-service).  

:::note
Because the `/var/lib/amaru` folder is owned by the user `amaru` you need to run commands using `sudo -u amaru`.  
Also the Amaru user is set to nologin for security concerns.
:::

## Firewall / nftables

If you want to use the monitoring stack to export Metrics / Logs and Traces, you will need to install Docker :

:::warning Docker interaction with nftables
Docker manages its own nftables/iptables-nft rules independently of `/etc/nftables.conf`.  
Before writing and applying your ruleset, keep in mind:

- **Rules set in the `chain input` do not apply to "internet -> docker" network traffic**  
Docker traffic goes through the `chain forward`, which is generally fully allowed or dropped.  
=> if the `chain forward` is not regulated a port published on `0.0.0.0` is exposed to the internet.

- **Allow Docker containers to reach the host.**  
Host-network scraping (e.g. Prometheus hitting host-level exporters) needs an explicit accept rule for the Docker bridge subnet, or it will be blocked by the default-drop input policy.
    ```bash title="/etc/nftables.conf"
    table inet filter {
    chain input {
       [...]
        # Allow Docker containers to reach the host
        ip saddr 172.16.0.0/12 accept
    }
    ```

- **Restarting the nftables service flushes Docker's rules too.**  
If you do restart nftables, you need to restart docker service too (run `sudo nft list ruleset` to see if Docker rules are loaded)
You can use `systemctl reload nftables` instead of restart. But be sure to include a dedicated `delete table inet filter` in your `/etc/nftables.conf` 
    ```bash title="/etc/nftables.conf"
    #!/usr/sbin/nft -f
    
    table inet filter
    delete table inet filter
    
    table inet filter {
    chain input {
        [...]
    ```
:::

<details>
<summary><strong>nftables Example configuration</strong></summary>

Uncomment parts of this conf to your need.

Remember to check your configuration with `sudo nft -c -f /etc/nftables.conf`

```bash title="/etc/nftables.conf"
#!/usr/sbin/nft -f

# Do not "flush ruleset" to protect docker's rules
table inet filter
delete table inet filter

table inet filter {
    chain input {
        type filter hook input priority 0; policy drop;

        iif lo accept
        ct state established,related accept
        ct state invalid drop
        ip protocol icmp accept
        ip6 nexthdr icmpv6 accept

        # Allow specifics IPs to SSH (recommended)
        # ip saddr { X.X.X.X, Y.Y.Y.Y } tcp dport 2222 accept
        # Allow all to SSH
        tcp dport 2222 accept

        # note: by default Amaru listen on :3000
        tcp dport 3001 accept

        # Allow Docker containers to reach the host 
        # (e.g. Prometheus scraping host-network exporters)
        # ip saddr 172.16.0.0/12 accept
    }

    chain forward {
        type filter hook forward priority 0; policy drop;

        # ct state established, related accept
        # ct state invalid drop
        
        # If you need to access your grafana dashboard 
        # Which is published on "80:3000"
        # You need to Allow forward traffic to :3000
        # ip daddr 172.16.0.0/12 tcp dport { 3000 } accept

        # Allow specifics IPs to join :443
        # ip saddr { X.X.X.X, Y.Y.Y.Y } tcp dport { 443 } accept
    }

    chain output {
        type filter hook output priority 0; policy accept;
    }
}
```
</details>

Once the host is hardened, continue to [Advanced Installation](05-amaru-advanced-installation.md) to install Amaru, create its systemd service, and run it.
