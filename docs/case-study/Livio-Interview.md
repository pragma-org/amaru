# Interview - Livio, SPO for the Cardano Foundation

Based on Damien's initial report, with a few additions:

### Next steps discussed:

Livio: send Damien/Arnaud a detailed list of useful node flags, behaviors, and pain points from his setup this week if possible.

Damien / team: use that list to refine September+ Amaru work, especially around operator quality-of-life features, documentation, and node-management UX.  
Arnaud / team: review how current node behavior and hidden/advanced flags can be created & explained in documentation.

--> Livio indicated that he has some time dedicated to experiment with Amaru, and would like to replace some relays with Amaru for an initial test. The low RAM consumption of Amaru nodes is a compelling factor, as he currently has to allocate around ~40GB of RAM to ensure a BP's stability.

### Overall documentation discussion:

Documentation should be practical and layered: a simple “first success” guide for preview/pre-prod first, then advanced sections for mainnet and deeper operational details.

There should be "advanced documentation" for each type of user of the node, and the simple version should be kept very simple. Different operator profiles: exchanges, large SPOs, and smaller operators likely need different levels of detail and reassurance.

Understanding the parameters to be set for the node can be done in a similar fashion as this: [https://jlopp.github.io/bitcoin-core-config-generator/](https://jlopp.github.io/bitcoin-core-config-generator/)

### Operational needs from Livio’s setup perspective:

- Having a way to measure/an indicator to see if all the blocks that were assigned to the pool in the leader schedule are produced the moment the leader schedule is known (currently a script)
- A way to switch between block producer instances without restarting
- Have a clear visibility into leadership schedule, block production status, missed blocks, and key rotation timing
- Have log filtering / log-level grouping so operators can focus on what matters

### Deployment reality:

Livio explained that their current production setup is VM-based, with custom scripts and Ansible, and that containers/Kubernetes were dropped because peer sharing exposed internal IPs.
Monitoring is done via Grafana Dashboard, Prometheus exporter, and custom script using cardano-db-sync to ensure that blocks are well forged.
Mithril is not used to deploy new nodes even though every pool has its Mithril signer. New nodes are deployed by rsyncing existing data.

### Key wishes from Livio:

1. Have versioned configuration files tied to node releases
   - Git repo (example): [https://github.com/cardano-foundation/cardano-configurations](https://github.com/cardano-foundation/cardano-configurations)
   - Have a compatibility analysis & List what has changed

2. A TUI that compiles the information into a dashboard of expectations, outcomes, failures, and successes  
   - "You will forge X" "You have forged X; Missed X..."

3. Cleaner log output with meaningful defaults and easier filtering by domain/importance.
   - Cleanup the information from the 'noise' VS the 'useful ones'
   - Network
   - Block production
   - Peer Level
   - Alert; Critical; Debug; Basic info
