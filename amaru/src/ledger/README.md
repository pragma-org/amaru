# Overview

Here below is a high-level mapping of the ledger state, extracted from the Haskell implementation and trimmed down to what is necessary to current era (Conway). This is written here for lack of better location, and mostly as a way to keep a mental image of the ledger state.

```
Ledger State
 ├─ UTxO (TxIn |-> TxOut)
 ├─ Stake distribution (StakeCredential |-> Lovelace)
 ├─ Certificate state
 │   ├─ DReps (DRepID |-> Epoch, Anchor, Lovelace, Set StakeCredential)
 │   ├─ Committee (CCID |-> Committee State)
 │   ├─ SPOs
 │   │   ├─ Current Parameters (PoolID |-> PoolParams)
 │   │   ├─ Future Parameters (PoolID |-> PoolParams)
 │   │   ├─ Retirements (PoolID |-> Epoch)
 │   │   └─ Deposits (PoolID |-> Lovelace)
 │   └─ Ada holders
 │       ├─ Deposits (StakeCredential |-> (Lovelace, Lovelace))
 │       ├─ Pool Delegations (StakeCredential |-> Option PoolID)
 │       └─ Gov Delegations (StakeCredential |-> Option DRepID)
 ├─ Governance State
 │   ├─ Proposals
 │   ├─ Committee
 │   ├─ DReps
 │   ├─ Constitution
 │   └─ Protocol Parameters (current, previous & future)
 ├─ Deposited (Lovelace)
 ├─ Fees (Lovelace)
 └─ Donation (Lovelace)
 ```
