
## Milestone 1: Onboarding tasks and general knowledge about Cardano (July)

### Identification of the key documentation and the different philosophies for setting up Cardano nodes :

- Developers.cardano.org - https://developers.cardano.org/docs/operators/node/running-cardano/

  --> Main documentation, up to date, a lot of explanation

- Coincashew - https://www.coincashew.com/coins/overview-ada/guide-how-to-build-a-haskell-stakepool-node

  --> Complete and well structured documentation, but outdated

- CardanoCommunity Guild-Operators - https://cardano-community.github.io/guild-operators/basics/

  --> Focus on SPO tools (guild-deploy.sh / CNTools / gLiveView / CNCLI / Mithril .. )

- Stakepool247/SPO - https://cardano-node-installation.stakepool247.eu/

  -->  A practical, copy-paste-ready guide for setting up and operating a Cardano stake pool

- Cardano-Course - https://cardano-course.gitbook.io/cardano-course/handbook
- CherryServers/Blogpost - https://www.cherryservers.com/blog/cardano-stake-pool
- Server hardening by ADANorthPool - https://adanorthpool.medium.com/server-harderning-and-optimization-for-the-would-be-node-operator-93242fd259dd


### Implementing the documentation :
- Use of an Ansible playbook to deploy the server hardening and monitoring referenced in these docs
- Set up a Haskell testnet then mainnet relay on a debian server (OVH Baremetal - 64GB ECC RAM - 1To SSD - 1gbps network)
	- Used Release binaries, then [Build with Nix](https://developers.cardano.org/docs/operators/node/installing-cardano-node/)
	- The initial sync on mainnet took around 72 hours..
	- Backed up and copied the mainnet db data (220 GB) to validate bootstrapping a new node without having to sync everything from scratch (bootstrap time is then driven by data transfer: 40 min rsync via network @100 MB/s -- 10 min rsync locally @375 MB/s)
	- Used [Mithril](https://developers.cardano.org/docs/operators/node/running-cardano/#bootstrap-with-mithril) to fetch signed snapshots (bootstrap time driven by downloading the data then replaying it: ~15 min of dl ; 35min total)
	- Used [gLiveView](https://cardano-community.github.io/guild-operators/Scripts/gliveview/) and Cardano-node Grafana monitoring dashboards.


### Experimenting with Amaru :
- Setting up Amaru Relays on testnet, preprod and mainnet
	- Used Release binaries, Nix Build, Cargo build and proposed OTLP docker monitoring to expose prometheus metrics.
	- Amaru bootstrap downloaded snapshot close to last epoch and had to recover its delay.
	- This process was quite slow for the initial sync of a Mainnet node, especially when compared to a Mithril sync using "`--include-ancillary`" which downloads the last ledger state. BUT :
		- Mithril snapshot download that includes the ledger state should not be used for a Block Producer node on mainnet anyway.
		- The snapshot provided by Amaru bootstrap was "old", so the syncing process had to catch up ~40 Epochs to reach the chain tip. An up-to-date snapshot will significantly reduce the initial synchronization time.

- Identified a [memory allocation issue](https://discord.com/channels/1202416088776712253/1231935912229994499/1534461220986552372) when syncing Amaru node on mainnet: after the initial bootstrap, the syncing process builds a huge memory footprint (around ~3GB) and keeps it when the node is finally running on tip. A node restart allows it to run with its original ~600MB memory footprint.
  --> Supporting Matthias's debugging by testing new memory allocation methods on a Linux environment.

### Stake Pool Operator Interview :
- [Markus Gufler](https://github.com/gufmar), SPO and Cardano ecosystem contributor : [Markus-Interview.md](Markus-Interview.md)
- Livio, SPO for the Cardano Foundation : [Livio-Interview.md](Livio-Interview.md)