---
type: process
status: accepted
---

# Record Engineering Decisions

## Context

Amaru is meant to be(come) a rather large project, spanning over multiple years. Along the way, decisions will be made. By the time one ponders the consequences of a particular decision, the people who made that decision may not be around the table anymore. This is true from a software architectural standpoint, as outlined by Michael Nygard in [Documenting Architecture Decision](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions), but also from a methodology standpoint. Why do we do things in a certain way? Why and how did a specific process was established around the project? Why this particular piece of technology was chosen?

Those are questions we ultimately want anyone to be able to answer when looking at the project. And this is only possible in the long-run if the decisions and the thought process behind are recorded to begin with.

## Decision

We will record all _important_ engineering decisions around the Amaru project. _Important_ is left to the judgment of the project maintainers but would entail anything that:

1. rely on a present context which may not endure;
2. has significance on the project structure and/or operations;
3. isn't immediately obvious.

Decisions are recorded as numbered (over 3 digits, left-padded) markdown files, versioned directly with the project sources. We currently recognise and record two types of decisions:

- `process`: decisions related to the development methodology or the overall process pertaining to the project.
- `architecture`: decisions that concern the software architecture at large (architecture style, protocols, interfaces, code organisation, interfaces, tools, etc).

### Template

As demonstrated by this decision record, we'll use the following template for all decisions moving forward:

```markdown
---
type: process | architecture
status: proposed | accepted | rejected
---

# <!-- title -->

## Motivation

<!-- Provide context, explain where the decision came from and why it's necessary to make one. -->

## Decision

<!-- Explain the decision with sufficient details to be self-explanatory. -->

## Consequences

<!-- Describe the result/consequences of applying that decision; both positive and negative outcomes -->

## Discussion points

<!-- Summarizes, a posteriori, the major discussion points around that decisions -->
```


## Consequences

- We'll start recording major engineering decisions on the project, like this one, before or after they happen. Conversations are often spread on different channels, especially Discord, in the present context, and we do not want decisions taken on peripheral channels to be lost in the void.

- Hopefully, as time will tell, this particular decision will help make the project _more Open Source_ as it will provide context and a trail of decisions to potential contributors (might it be our future self).


## Discussion points

- We looked at [Hydra](https://hydra.family/head-protocol/adr/) & [Ogmios](https://github.com/CardanoSolutions/ogmios/tree/master/architectural-decisions/accepted) as projects that successfully used archiectural decision records to capture decisions.

- We mentioned how few people in the Aiken community regret not having ways to track decisions made around the project. Many conversations get lost on Discord and newcomers have usually no ways to know that some matters got extensively discussed in the past.
