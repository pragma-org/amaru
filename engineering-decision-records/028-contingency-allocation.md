---
type: process
status: accepted
---

# Allocating the contingency to must have items of the 2026 Amaru roadmap

## Context

Our budget has been built with a 0.5$ value for ₳ and with a contingency at 0.3$.
Given the current trend on the market (₳ below 0.3$ since February 1st 2026) we need to calculate and act on a worst case scenario in order to keep the main delivery targets relevant.

## Decision

This section describes:
- The scope that the project team deemed `absolutely necessary` to be delivered in 2026
- The calculation of the amounts that need to be secured
- The final decision regarding contingency allocation 

### Must have scope list from the 2026 targets

The following activities and people were included in the calculation of the `absolutely necessary`:
- Current ongoing contracts: 3 Core development contracts, 1 Operations & Use cases contract
- Core development: Consensus, Mempool and simulation development (Eric)
- Core development: Ledger development (Josh)
- Core development: Consensus, Networking and simulation development (Roland)
- Network Compliance & Cardano-level Testing: White hacking and advanced testing (Jon)
- Operations & Use Cases: Project, Product management and facilitation (Damien)
- Operations & Use Cases: Node diversity workshop costs (Porto, June)
- Operations & Use Cases: Devops & SPO management (Arnaud)

This scope will help us deliver in 2026:
- Feature parity with the Haskell node for a relay node on mainnet; emphasis on the user experience (SPO & DApps dev.) and with a very low resource footprint
- Feature parity with the Haskell node for a block producer node on mainnet; emphasis on the user experience (SPO & DApps dev.) and with a very low resource footprint
- Improved testing suite for Cardano nodes and better documentation of the behaviours of the Cardano network and existing node implementations

### Amounts to be secured based on the must have scope

Here are the amounts that we estimated to be covered for the rest of 2026:
- Current ongoing contracts: 3 Core development contracts, 1 Operations & Use cases contract - $210337 & ₳204000 
- Core development: Consensus, Mempool and simulation development (Eric) - $168000
- Core development: Ledger development (Josh) - $168000
- Core development: Consensus, Networking and simulation development (Roland) - ₳563500
- Network Compliance & Cardano-level Testing: White hacking and advanced testing (Jon) - $150000
- Operations & Use Cases: Project, Product management and facilitation (Damien) - ₳1150000
- Operations & Use Cases: Node diversity workshop costs (Porto, June) - $48462
- Operations & Use Cases: Devops & SPO management (Arnaud) - $108000

<br>

This gives us the amounts per scope to be paid in 2026:
- Core development: $546337 & ₳620000
- Network Compliance & Cardano-level Testing: $150000
- Operations & Use Cases: $156462 & ₳1259000

<br>

We defined a target of 0.167$ for the minimum ₳ value that can secure us the scopes mentioned, every scope owner has to trade (at least) to that level to secure the amounts to be paid to our partners.  
Anything traded above that threshold will be owned by the scope owner to allocate as he decides.

## Consequences

Given the decision made, we prepared the [following transaction](https://cardanoscan.io/transaction/68ec097ba1fc006f236d57450361f9a6577bf59f7a0b1c0259b30b4d3b03630e) using this [IPFS document](https://ipfs.io/ipfs/bafkreibfgoyo5jg3ufd3jeqg2mhjo5e2wdscfpfqqei3rkb7pmndj5mzem) to document the following contingency movement:
- Contingency: -₳3851692.84
- Core development: +₳1556478.04
- Network Compliance & Cardano-level Testing: +₳898203.59
- Operations & Use Cases: +₳1397011.20

## Discussion Points

In that calculation we did not take into account the Middleware Development scope for which we have ₳900000 that we keep as an additional contingency in worst case scenario.

## References

