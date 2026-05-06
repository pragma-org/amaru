---
type: process
status: accepted
supersedes: ./016-treasury-management-acceptance-process.md
---

# Treasury Management & Acceptance Process

## Context

This decision record documents the process behind managing the payments linked to the treasury budget.

We want to have a very straightforward approach to managing the treasury budget we receive and the acceptance of deliverables made on the project.

Amaru's operating model builds a set of contracts that are milestone based, we must have a common way of both reviewing the work and triggering the payment linked to the milestones set in the contract.

Thus, we would like to track and report activity monthly in a very simple manner to have a transparent and coherent treasury management process.

We also have some Time & Material agreements, but they should follow a similar principle for managing each monthly payment with adaptation made to review hours spent.

## Decision

We will use a monthly review for each existing contract to assess, every contract active will have a monthly review that will cover the following:
1. Gather the initial Milestone content from the signed contract;
2. Review delivered or pending activities (in Github repo)
3. Draw out a compiled list of accomplished deliveries
4. List out the necessary activities to be prioritized over the next month
5. Deliver a “Proof of Acceptance” and trigger the payment of the milestone

Once the work has been deemed acceptable, the contributor has to generate an invoice to:

```
Amaru Maintainer Committee
Dammstrasse 16, 6300 Zug
Switzerland
```

The invoice should mention the scope and the work accomplished and send it via email with as cc:

  - Laura Dugan <laura@cag.xyz>
  - Damien Czapla <damien.czapla@openthelead.com>

When receiving this invoice, the scope owner will do a `disburse` and the various proceedings behind the currency of payment specified in the contract will be enacted. The address to be used for the disburse is:

```
addr1q8qrds2nnx7clx3kcpp2l0eu45twmdcahsfu9m0xcwy59j6xz3vs0hnfaz9nhje8z34kfnds4jyk7hs6dnrag6e2lfgqtyf4rl
```

The acceptance process is deemed finished when the scope owner (accountable of the work done) updates the [amaru-treasury Journal][] with an entry stating that he accepts the work done and includes all the details related to the payment in the metadata of the transaction (IPFS of the contract, IPFS of the invoice, description of the intent)

In that process the people involved are:
- The contributor (or a representative from the company that knows about the activities accomplished) as the main responsible for work done,
- The scope owner as the guardian of the acceptance of the results,
- The project manager of the project as the facilitator of the overall meeting.

Note for Time & Material contracts, at the review of the Step #2 there will be a look into the hours spent on each topic mentionned.

Every scope of work bound by a contract on the project shall be subject to the acceptance process mentioned here.

## Consequences

The [following document](https://ipfs.io/ipfs/bafybeicqqdq2uzzjfi6xjqjy7segwtgyn2woq2wqikcr4rmjzfzluj4lta) describes each step of the process in a very simple analysis that covers for each step:

- **Content**: The action required
- **Inputs**: The sources used for the action
- **Conditions**: The parameters that will influence the content of the action
- **Outputs**: The necessary outcome that is required for the action to be deemed complete

Each contracted work done on the project will be available in the [amaru-treasury Journal][] in a transparent manner so that everyone is aware of what's ongoing.

## Discussion points

- Last year's payments were having difficulties being timely.
- There is a need of a more specific cadence to keep up the prioritization of tasks and also have a regular payment cycle on the project.
- Up until now this process was left at the appreciation of the scope owner, we need some kind of a common way of doing things AND a simple process anyone can follow.

[amaru-treasury Journal]: (https://github.com/pragma-org/amaru-treasury/tree/main/journal)
