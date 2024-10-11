---
type: process
status: accepted
---

# Amaru design framework

## Context

Amaru is meant to be(come) a rather large and complex project, spanning over multiple years. 

The ambition of the project is to build a new fully interoperable block-producing node for improving the overall performance the Cardano blockchain. 

The Amaru node wants to provide a simplified entry point for building things on Cardano by using a modular design and Rust as its main coding language.

## Motivation

Given the ambition of the project, we need a structured approach to turn ambitions into concrete goals.

We want to have a unified structure to streamline development, ensure cross-team collaboration, and maintain focus on core objectives. 

This framework needs to enable efficient coordination across diverse teams and ensuring that all contributors align with the product’s high standards for performance, security, and resilience. 

## Decision

Amaru will be using a customised framework adapted to the setup of the project :

[![Amaru](https://i.ibb.co/NrZ47nP/Capture-d-cran-2024-07-04-170154.png)](https://pragma.builders/projects/amaru/)
_Representation of the development framework that will be used throughout the life of Amaru_

This representation encapsulates all the phases imagined for running the Amaru project :
1. Problem framing : [DDD : Framing the problem](https://miro.com/app/board/uXjVNpa1sM0=/?fromEmbed=1) linking together Problem, People, Outcome and Constraints
2. Solution architecture : [C4 Architecture](https://miro.com/app/board/uXjVNpawiPE=/?fromEmbed=1) representing the system and the interfaces between each key components
3. Solution flows and structure : [DDD : Bounded context](https://miro.com/app/board/uXjVNpa36mI=/?fromEmbed=1) focusing on use cases and structuring the delivery blocks of the solution
4. Manage release plans & interfaces : [Extreme programming](https://en.wikipedia.org/wiki/Extreme_programming) build a cycle with a release plan directed towards delivering a demo
5. Demonstrate : [Extreme programming](https://en.wikipedia.org/wiki/Extreme_programming) deliver on the features with acceptance criteria and KPIs to measure
6. Reflect and learn : [Lean Startup : Validated learning](https://theleanstartup.com/principles) allocate time to confront the solution to the problem environment and its user
7. Final solution deliver : [Lean Startup : Build, Measure, Learn](https://theleanstartup.com/principles) document the main delivery and discoveries related to the problems, prepare the next maintaining cycle

### Result of the end of phase 3: Solutions flows and architecture

Here is the list of the scopes that have to be delivered following the domain driven design: bounded context approach applied to our project.

![Scopes](https://i.ibb.co/4NDJKnb/Picture1.png)

For each scope identified, we nominated an owner that has the responsibility of managing the interfaces and the coherence of its scope:
- Ouroboros Consensus owner: Arnaud Bailly
- Cardano Ledger owner: Matthias Benkort
- Testnet facilitator owner: Chris Gianelloni
- Peer 2 peer (P2P) owner: Santiago Carmuega
- Dolos owner: Santiago Carmuega
- UTxO RPC specifications owner: Chris Gianelloni
- GO Node owner: Chris Gianelloni
- Amaru integration: Amaru Maintainers Committee

This will be updated once we align on the structure for phases 4; 5; 6 that will drive the deliveries linked to Amaru.

## Consequence

- This framework will apply to each scopes included in the Amaru project
- The scope owners have the responsibility to apply, break and update the content of this framework
- This will provide a documented overview of the product scopes and interfaces

## Discussion points

- We looked at various sources for encapsulating the _minimum structure necessary_ to structure the project

> * [Domain Driven Design modelling process](https://github.com/ddd-crew/ddd-starter-modelling-process/blob/master/README.md)
> * [Core mindset behind each step of Drive & Deliver](https://theleanstartup.com/principles)
> * [Extreme programming is the guideline for the Drive & deliver steps of the framework](https://www.altexsoft.com/blog/extreme-programming-values-principles-and-practices/)
> * [Outcome driven methodology used to create the framework for Amaru](https://www.mobiusloop.com/blog/pka8i66gimn35593mck8f4ipwidenb)  

- We integrated feedbacks of experienced software developpers, project managers, product managers into our approach and kept just the necessary phases
