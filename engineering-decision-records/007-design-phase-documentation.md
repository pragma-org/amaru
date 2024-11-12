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

<img src="https://i.imgur.com/5NLPQ7t.png"
     alt="sample image"
     style="display: block; margin-right: auto; margin-left: auto; width: 45%;
     box-shadow: 0 4px 8px 0 rgba(0, 0, 0, 0.2), 0 6px 20px 0 rgba(0, 0, 0, 0.19)" />

The main driver will be to bring operational resilience, meaning our design and architecture must be robust and improve the security aspect of the Cardano network.

The most discussed problem during our sessions was about the resources it takes to run a Cardano node today, this is currently a concern for most SPOs and we will need to solve that by making it the least demanding we can. 
This also goes with the fact that current running a node on an ARM setup isn't easy, we want to solve that by being able to run the node with low cost, low consumption, low heat generation setups.

Then the aspect of contribution, experimenting and customizing the node became the next topic to be discussed for people that currently run a node and the ones that build on top of it. The main concern was targeted towards having a closer look of the node inner-working to modify, select only the required setup to run for any use case. The modular approach of our architecture is directly linked to these concerns and will be one of our main decision making criteria.

### People impacted

Next on the menu was identifying who are the people that are in the environment of Amaru.

![Problem statements](https://github.com/user-attachments/assets/4db86523-6edd-4d8c-b590-c42b27f62539)

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

![Maintainers](https://github.com/user-attachments/assets/6f27ba9e-fa87-4cfa-b03e-65efb8a38f91)

For each scope identified, we nominated an owner that has the responsibility of managing the interfaces and the coherence of its scope:
- Amaru integration: Amaru Maintainers Committee (*to be re-allocated*)
- Ledger owner: Matthias Benkort
- Consensus owner: Arnaud Bailly
- Dolos owner: Santiago Carmuega
- Forge owner: (*to be allocated*)
- GO Node owner: Chris Gianelloni
- Mempool owner: Andrew Westberg
- Management RPC: (*to be allocated*)
- Peer 2 peer (P2P) owner: Santiago Carmuega
- Testnet facilitator owner: Chris Gianelloni
- UTxO RPC specifications owner: Chris Gianelloni & Santiago Carmuega

This will be updated when we have start delivering and better understand the product.
