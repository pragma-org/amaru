---
type: process
status: accepted
---

# ADA-only Contracts with Amaru

## Context

We have contracts valued in USD and in ADA. However, the price of ada can be quite volatile and uncertain. While this is "part of the game" when being paid in ada, we are also conscious of the risks that come with using ada as a mean of payment. We thus want to offer _some level of stability_ to contractors paid in ada; especially in times of constant downwards market trends.

This EDR also serves as a public reference point for the decision pertaining to recently signed (June 2026) contracts.

## Decision

### Contract valuation and reviews

Each contract paid in ada is valued according to the following rules:

1. Contracts are signed for and reviewed every 6 months;
2. We consider the equivalent dollar amount for an FTE valued at $225k (as per our budget);
3. For conversion rate, we use the lowest ada value of the previous semester.

Hence for the contracts signed on the 08th of June, that covers the period from July to December;
we used the lowest price of ada over the period [08 Dec 2025, 08 June 2026] which was 0.15 at the time of the review.

### Contract environment storage

The 2026's Amaru treasury journal contains [an inventory of the draft and signed contracts](https://github.com/pragma-org/amaru-treasury/tree/main/journal/2026/contracts).

Each scope owner is accountable for defining the scope and the amounts of each contract related to his scope and its deliveries.

Each contract is then reviewed and co-signed by all 4 scope owners. A contract is deemed active once all 4 scope owners have signed.

### Treasury status

The real-time status of the Amaru treasuries can be viewed on [https://amaru-treasury.plutimus.com/](https://amaru-treasury.plutimus.com/). This view may eventually be integrated to the [main Amaru website](https://amaru.global) once time allows.

Additionally, the [Amaru-treasury Journal](https://github.com/pragma-org/amaru-treasury/tree/main/journal/2026) provides an aggregated view of the various transactions that went in and out of each sub-treasuries.

## Consequences

- Two recent contracts signed in June (_Open The Lead_ & _RKSW UG_) have used a conversion rate of `$0.15` per ada.
- In that calculation we did not take into account the Middleware Development scope for which we have ₳900000 that we keep as an additional contingency in worst case scenario.

## Discussion Points

N/A
