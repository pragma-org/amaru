---
type: architecture
status: accepted
---

# Design phase documentation: Problem statements, stakeholders and architecture

## Motivation

Amaru is meant to be(come) a rather large project, spanning over multiple years.
Given the ambition of the project, we need a structured approach to turn ambitions into concrete goals.
We want to have a unified structure to streamline development, ensure cross-team collaboration, and maintain focus on core objectives. 

## Decision

We chose to apply the customised framework we designed, this decision record is the detailed documentation of the design phase first 3 steps: Problem framing, solution architecture, solution flows & structure

## Consequences

Our objective during the Design phase was to create clear and compelling problem statements that establishes a starting point to quickly understand : 
> Who is impacted by our project,
> 
> The desired outcomes that define success for our project
> 
> The constraints that will need to be taken into account

While starting out the discussion one topic was clear: we need diversity in the people contributing right from the start. 
The initial team that contributed to this design phase came from 6 companies: Blink Labs, Cardano Foundation, dcSparks, IOHK, Sundae Labs, TxPipe. 
The roles included were also of a varied range: CEO, COO, CTO, Senior developer, Project manager, SPO. 

### Problem statements

The first task at hand was formalising "what problems are we trying to solve by starting Amaru?"

![Problem statements](https://github.com/user-attachments/assets/ae21dfbf-5f1a-4bac-8ca7-8b6603680546)

The main driver will be to bring operational resilience, meaning our design and architecture must be robust and improve the security aspect of the Cardano network.

The most discussed problem during our sessions was about the resources it takes to run a Cardano node today, this is currently a concern for most SPOs and we will need to solve that by making it the least demanding we can. 
This also goes with the fact that current running a node on an ARM setup isn't easy, we want to solve that by being able to run the node with low cost, low consumption, low heat generation setups.

Then the aspect of contribution, experimenting and customizing the node became the next topic to be discussed for people that currently run a node and the ones that build on top of it. The main concern was targeted towards having a closer look of the node inner-working to modify, select only the required setup to run for any use case. The modular approach of our architecture is directly linked to these concerns and will be one of our main decision making criteria.

### People impacted

Next on the menu was identifying who are the people that are in the environment of Amaru.

![People impacted](https://github.com/user-attachments/assets/b23bee65-36a5-4660-ab63-0fc52cd25b96)

No surprises in the people impacted by a Cardano node, Developers & SPOs will be our users, and our customers will be DApps owners and Services Providers that uses the node in their setup (e.g. DEX)

Our project will maintain a close relationship with all the people identified here as part of our development process and keep an eye on the use cases that are required of Amaru.

### Desired outcomes: People

For this category we asked ourselves what success would mean for people using Amaru, and the outcome was comforting the vision we had. 

This will serve to draft out metrics that will drive our decision making towards meeting those success criteria.

![People desired outcomes](https://github.com/user-attachments/assets/960e2e5f-1289-4722-b4dd-796ae486542a)

Here are two examples of the KPIs that we will monitor with the coming releases of Amaru thanks to the above results:
> Resources consumption of running Amaru VS the Cardano Haskell node
> 
> Cost of ownership of running a Node (SPO focused)

### Desired outcomes: Business

Now we looked at what success meant for the current market in which the Cardano node is.

As mentioned above this will provide valuable data to be gathered to identify if Amaru is going the right direction in its market.

![Business desired outcomes](https://github.com/user-attachments/assets/240172b1-a6de-4008-a810-0f460ef31448)

Here are two examples of the KPIs that we will monitor with the coming releases of Amaru thanks to the above results:
> Percentage split of SPOs running Amaru on the network
> 
> Number of services/dApps integrating Amaru

### Constraints and risks

As our final step before diving into the architecture of the solution we looked at the things we'll have to integrate as constraints and the potential risks that might come up on our project.

The outcome of this will drive our synchronisation steps and the evolutions we plan for Amaru.

![Constraints analysis](https://github.com/user-attachments/assets/33996d90-a677-4298-a08e-5f7ab6d1913c)

Here is a list of things that are now integrated in the project:
> Testing strategy for each pull requests and major releases
>
> Rust focused peer reviews included in the development process 
> 
> Code audit and security audit

To finalise our problem framing we ended up starting to draft risks that might impact the project.

![Risk Analysis](https://github.com/user-attachments/assets/67c6a3fe-4a8b-4bb2-95a3-e719c22ce7af)

This initiated a risk register for the project and began mitigation plans related to the risk identified, here is an example of 2 mitigation plan ongoing:
> Senior Rust advisor regular sanity checks on the code base built
>
> Alignment on the Cardano Node next milestones and architecture modifications (working groups)

### Architecture C4 diagram

The next step towards building the product architecture was to use the [C4 Architecture](https://miro.com/app/board/uXjVNpawiPE=/?fromEmbed=1)

![Context](https://github.com/user-attachments/assets/d55a8c9c-e16f-43fe-93b2-3d634c58c20e)

Level 1 of the C4 methodology is about showing how the system fits into the world around it, here we can see the 3 main interaction points for Amaru:
- Upstream nodes
- Downstream nodes
- Client Apps

Now if we dive a bit deeper into the system for Level 2, let's look at the containers and data stores inside our node:

![Containers view](https://github.com/user-attachments/assets/c826ab17-c8fb-41e4-980b-8bdff60fdf27)

Final step that we went through with the C4 model is the level 3: diving into the components inside each container.

Consensus container:
![Consensus container](https://github.com/user-attachments/assets/fbe9a00d-5cf6-4e03-8016-a3f578013b3a)

Peer 2 Peer container:
![Network container](https://github.com/user-attachments/assets/47cb1c15-55e7-4561-a35c-6ce2d8b28af3)

RPC container:
![RPC Container](https://github.com/user-attachments/assets/fc52c608-ce39-4618-b7ca-edc1f10e7644)

Transition container:
![Transition container](https://github.com/user-attachments/assets/23642f86-522c-4ba6-a9b5-283252563940)

This level was sufficient for us to go to the final step of our design phase which is choosing bounded context and making scopes of the project that can be autonomously run by a team. 

### Bounded context alignment

The final design step was turning this architecture of the product into actionable parts that can be owned by a dedicated development team. 

While thinking about that representation we used the bounded context methodology of Domain Driven Design to represent relationships, shared kernels and interfaces that will have to be monitored by each team that owns the bounded contexts. 

![Bounded contexts](https://github.com/user-attachments/assets/3a1d3b6d-ff2f-44ba-8f4f-c3fcf4d722bd)

In this representation you can find the following types of links featured:
- SK (Shared Kernel): contains code and data shared across multiple bounded contexts within the same domain
- CF (Conformist): the downstream team must accept and adapt to the upstream team’s decisions
- OHS (Open-Host Service): the supplier decouples its implementation logic from its public API to better serve consumers (can be subject to multiple integrations)
- PL (Published Language): part of the domain that is exposed by the upstream member

This fuelled our discussion on which part to dedicate a team for our project and we came up with the following independent teams (that might be subject to change as the project evolves):
![2025_04_13_Amaru_Scopes](https://github.com/user-attachments/assets/9e9a0fee-7032-40c6-a4a6-d0d0a7108ace)


In this representation the blue scopes are considered scope owners of the project, they are part of the Maintainer Committee and have decision each a voice for development decision making and integration.  

For each scope identified, we nominated an owner that has the responsibility of managing the interfaces and the coherence of its scope:
- Amaru integration: Amaru Maintainers Committee (with final deciding member: [Matthias Benkort](https://github.com/ktorz))
- Ledger owner: [Matthias Benkort](https://github.com/ktorz)
- Ouroboros Consensus owner: [Arnaud Bailly](https://github.com/abailly)
- Mempool and block forge owner: [Pi Lanningham](https://github.com/Quantumplation)
- Peer 2 peer (P2P) owner: [Santiago Carmuega](https://github.com/scarmuega)
- Operator Interface Management: refining the scope content and timeline, no owner yet nominated

> [!TIP]
> The mentioned owners are decision makers for the technical progress of the project and the integration of the various scopes into Amaru.  
> For the decision making on "paid contributors" and allocation of our budget please refer to the Amaru treasury proposal.

For each of the scopes that are linked with our project but aren't hosted in the Amaru repo we identified a "go to" contact for each interface that impacts our development:
- Dingo (GO Node) owner: [Chris Gianelloni](https://github.com/wolf31o2) see: [Dingo Github](https://github.com/blinklabs-io/dingo)
- Dolos owner: [Santiago Carmuega](https://github.com/scarmuega) see: [Dolos Github](https://github.com/txpipe/dolos)  [Dolos proposal](https://gov.tools/budget_discussion/39)
- UTxO RPC owner: [Santiago Carmuega](https://github.com/scarmuega) see: [UTxO RPC Github](https://github.com/utxorpc) [UTxO RPC proposal](https://gov.tools/budget_discussion/40) 
- Pallas owner: [Santiago Carmuega](https://github.com/scarmuega) see: [Pallas Github](https://github.com/txpipe/pallas) [Pallas proposal](https://gov.tools/budget_discussion/41)
- UPLC VM owner: [Lucas Rosa](https://github.com/rvcas) see: [UPLC Github](https://github.com/pragma-org/uplc)

### Treasury proposal: scope owners and budget administration

The Amaru team submitted a proposal with a list of scopes related to the project and describing a specific way of administrating the budget, this is now reflected into the ways of working of the project.

#### Scopes described in the proposal

![2025_04_13_Amaru_ask](https://github.com/user-attachments/assets/041dacde-5717-4b6f-87a8-1fc279d47e23)


> [!TIP]
> The scopes described here are what the Amaru team expects to deliver in Q3 & Q4 2025
> For example the Networking is not included as it will already be delivered, but some integration might be required, that's the purpose of the "Ad-Hoc mercenaries"

Scope | Owner | Estimated effort | Resources already secured | Resources needed
---   | ---   | ---                | ---                       | ---
Ledger | Matthias | 2.5 FTEs | 1 FTE | 1.5 FTEs
Consensus | Arnaud | 2.5 FTEs | 1 FTE | 1.5 FTEs
Networking | Santiago | 1 FTE | 1 FTE | 0 FTEs
Ad-hoc Mercenaries | Pi | 2.5 FTEs | 0 FTEs | 2.5 FTEs
Project Management, Public Relations & Marketing | Damien | 0.5 FTEs | 0 FTEs | 0.5 FTEs

#### New scopes from the proposal

##### Ad-Hoc mercernaries


**Purpose:** troubleshoot the gaps not planned for and articulate smart problem solving on the project.

Examples of activities:
- Troubleshoot integration activities
- Interface
- Specific use case development, testing and delivery

One of the scopes that will be carried by the Ad-Hoc team will be the Mempool as it requires strong interfacing with the rest of the team.
This includes:
- Mempool library: Build a standalone library (data structure that represents transactions in memory) with an interface to get an extract of these transactions
- "Simple" mempool implementation: Build a basic mempool that adds, remove, gather, drain transactions and exposes a new ledger state
- "Complex" mempool implementation: Build a more refined version of the mempool that handles features differently and optimises the overall resources consumption
- Mempool tooling and API: Create a tool able to manage the mempool and the modularity
- Node management Remote Procedure Call (RPC): Build a software that handles the "operator perspective" on operating the Amaru node
- Block forging: Develop a component able to forge blocs

Timeline:
- Mempool library: done
- "Simple" mempool implementation: Q2 2025
- Block forging: Q3 2025
- "Complex" mempool implementation: Q4 2025

#### Project Management, Public relations and Marketing

**Purpose:** drive key initiatives and manage use cases and end users feedback loops. 

The current set of activities forecasted are: 
- Creating and managing SPO working groups to actively improve the operator side of Amaru 
- Organizing and facilitating workshops related to key explorations of the product
- Marketing, implementation partnerships alignment, use case specific collaborations
- Roadmaps interfaces and scopes alignment synchronization 
- Bug bounty: create an incentive to generate traffic on the testnet preprod and bring contributors to the project
> *Testnet developer bounty:* Create on preprod the use cases defined by the Amaru team by building the transactions on the network and generate the conditions needed  
> *Testnet SPO bounty:* Produce a block that includes the above use case   
> *Amaru development bounty:* be involved in at least 3 contributions accepted by maintainers   

Our current ambition is to experiment with Amaru as much as we can with testnet activity, conformance tests, simulations and build (with the feedback of early adopters) a reliable setup that fullfills all our global targets.

Each bounded context will share ownership over a resource that will interface with all the stakeholders of the Amaru project to facilitate the activities mentioned above.

Timeline:
- SPO working group setup and key initiatives: Q3 2025
- Bug bounty setup and start: Q3 2025
- Amaru workshops: Every quarter


#### Administration of the budget

The maintainer committee will ensure **direct administration** of the budget, assisted with **an on-chain smart contract** (developed in open source, still incomplete, but aimed to be done and fully audited by the time of the first withdrawal). The smart contract's role is to ensure that the expenditure of funds is done in accordance with the scope defined in this budget and authorized by the relevant scope owners. 

We recognize the following capabilities of this contract:

1. **Standard withdrawal**: A scope owner asks other scope owners for money to be withdrawn from his scope.
2. **Contingency withdrawal**: A scope owner asks other scope owners to withdraw an amount from the contingency funds.
3. **Scope reconciliation**: A scope owner asks other scope owners for a change of ownership (or a reallocation of budget).
4. **Contingency refund/closing**: scope owners ask to send the leftovers from the contingency budget to be sent back to the Cardano treasury.
5. **Credential rotation**: In case of lost credentials or the departure of a scope owner, a mechanism allows the rotation of credentials to a new scope owner upon approval by all (5 out of 5) PRAGMA members (effectively capturing PRAGMA's board decision to appoint new maintainers).
6. **Failsafe**: In the extreme scenario where credentials would be irremediably lost, thus preventing any further decision, a failsafe mechanism allows all unconsumed funds to be sent back to the Cardano treasury.   

#### Acceptance process for milestones and deliverables on the project

Any milestone or delivery item will have to go through this process:
- Submit a pull request to the Amaru repo on the main branch
- Demonstrate evidence of testing: once implemented in the Amaru context, make reports available to show that the expected features are working within the targeted environment.
- Formally review with the Amaru Maintainer Committee the pull request (in the recurrent bi-monthly maintainer meeting or in a dedicated meeting)
- Finalise with the scope owner the integration and merge the pull request into the Amaru repo

## Discussion points

This documentation will be updated when we better understand the product and our project environment after each delivery step.
