Nov 14, 2024 | Amaru - Mempool Interface Architecture
Attendees:  Andrew Westberg     Pi Lanningham  ethan@sundaeswap.finance Damien CZAPLA

Notes
Avoid flood attacks where someone could flood the mempool with invalid txns.
Mempool might be used outside of a “node” - sundae scooper.
Someone might want to implement node-level MEV. (re-ordering the mempool to extract extra value via arbitrage, etc…). For example, Andrew runs a pool and is also a Sundae Scooper. He moves his scoops to the top of the mempool to make sure he puts his txns into a block.
How do we prepare for Leios?
How do we prepare for Validation Zones (use case: babel fees)?

Brainstorming Ideas

“Simple Mempool”
Add Transaction - TRAIT
Validate Transaction against some “Ledger State” (virtual)
Separate implementation for adding a transaction locally vs. network.
Remove Transaction(s) - Optional TRAIT with noop default implementation
Current mempool does not allow manual removal of txns.
Does this need to be in the public API?
Is this a primitive where it’s used by Gather to remove 1 tx at a time?
Gather/Drain transactions for Block - TRAIT?
Validate Size in Bytes
Validate CPU Units
Validate Memory Steps
Expose a list of valid transactions that can be used in the block.
Open question - Do we grab pre-validated txns, or validate at gather time?
Expose a new Ledger State (a new block arrives) - TRAIT
Mark transactions as applied.
Maybe or Maybe not remove a tx immediately.
Possibly use Expose a new ledger state during rollbacks by re-applying any transactions that got rolled back.
Mempool only “knows” about transactions and ledger state, not the chain, blocks, etc…
Share transactions with peers
Customize how txns are exposed to others
Expose mempool state to Developers

“Complex Mempool”
Multi-Tip monitoring
Being able to submit transactions that aren’t valid “yet”.
Bin-packing
Maximize all 3 dimensions of resource consumption
Simple example - fill up bytes after either CPU or Mem are exhausted in a block.
Floating Transactions - Some transactions always bubble to the top of the mempool.
Tiered Pricing - Sortability of the mempool based on fee or some other metrics
Block itself reserves a certain amount of capacity for high-priority txns.
Secret Transactions that don’t propagate - keep this in mind.
Mark instead of Delete transactions so rollbacks aren’t catastrophic.
Disk storage - persistent mempool?
Statistics or Data analysis of the mempool.
How full is the mempool on average…
Prometheus metrics
Cpu & memory doesn’t exist today

