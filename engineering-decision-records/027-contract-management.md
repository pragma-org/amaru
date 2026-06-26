---
type: process
status: accepted
---

# Contract management and evaluations on Amaru

## Context

We have contracts valued in $ and in ₳ and we want to adjust the value of the ₳ contracts to be relevant to the conditions of the market.  

The current market trend (₳ being under 0.3$ since February 1st) made us rethink our approach and valuation of contracts.  

Given the risk that goes with using ₳ as a mean of payment for our partners we decided to organise on the first week of June an overview of the existing contracts and their value compared to the market.  

We also wanted a specific public reference point for the existing contracts and the status of the budget.

## Decision

This section describes:
- A rule about contracts valuation and reviews
- An environment to store active and inactive contracts
- A direct source to check on the smart contract scopes status

### Contract valuation and reviews

For each contract valued in $ we confirmed the partners were still on board with the amounts initially settled at the start of the year for the upcoming months.  

For each contract valued in ₳ we redefined the value of those contract based on two facts:
1. FTE for our budget is valued at $225k
2. Lowest ₳ value of the last 5 months  

Given those parameters we reviewed all the existing contracts and drafted new ones that starts in June.

For the two contracts we have valued in ₳ we took 0.15$ as a lowest ₳ value. 

### Contract environment storage

The resulting contracts that were drafted and signed after that review [can be accessed in the Amaru-treasury repository](https://github.com/pragma-org/amaru-treasury/tree/main/journal/2026/contracts)

Each scope owner is accountable for defining the scope and the amounts of each contract related to his scope and its deliveries,  
The contract is then sent for review to all 4 scope owners.  
A contract is deemed active once all 4 scope owners have signed.  

The current list of active contracts that can be found [there](https://github.com/pragma-org/amaru-treasury/tree/main/journal/2026/contracts) is:
- Core development: Consensus, Mempool and simulation development (Eric - Sundae Labs)
- Core development: Ledger development (Josh - Sundae Labs)
- Core development: Consensus, Networking and simulation development (Roland - RSKW UG)
- Network Compliance & Cardano-level Testing: White hacking and advanced testing (Jon - Cyber Castellum)
- Operations & Use Cases: Project, Product management and facilitation (Damien - CZ Venture)
- Operations & Use Cases: Node diversity workshop costs (Porto, June - Open the lead)
- Operations & Use Cases: Devops & SPO management (Arnaud - Wolf Pxl)
- Operations & Use Cases: Accounting 2026 (Crypto Accounting Group)

### Smart contract status

To access the on chain status of the treasury smart contract Paolo built [this website](https://amaru-treasury.plutimus.com/) that helps you inspect the live information of each scope.  

Also you can find in the [Amaru-treasury Journal](https://github.com/pragma-org/amaru-treasury/tree/main/journal/2026) all the history of the transaction made 

## Consequences

[Contract database](https://github.com/pragma-org/amaru-treasury/tree/main/journal/2026/contracts)  
[Amaru treasury - Status](https://amaru-treasury.plutimus.com/)  
[Amaru treasury - Journal](https://github.com/pragma-org/amaru-treasury/tree/main/journal/2026)  

## Discussion Points

In that calculation we did not take into account the Middleware Development scope for which we have ₳900000 that we keep as an additional contingency in worst case scenario.

## References
