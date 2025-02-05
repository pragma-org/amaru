---
type: process
status: accepted
---

# Logbooks

## Context
We introduce the notion of a "Logbook" to complement the [Engineering Decision Records](./001-record-engineering-decisions.md) process by capturing developer musings, notes, tips, and informal documentation throughout the development process.

## Motivation
[Engineering Decision Records](./001-record-engineering-decisions.md) are used within this repository to record official decisions that we reach as a team. However, they do not capture all context that might be useful to capture for a highly exploratory project like this one.
For example, it would not be appropriate to record experiments, helpful tips, meeting notes, or other informal but highly valuable contextual information about development.
We take inspiration from the [Ouroboros Leios](https://github.com/input-output-hk/ouroboros-leios/blob/main/Logbook.md) and [Ouroboros Peras](https://github.com/input-output-hk/peras-design/blob/main/Logbook.md) projects who instituted a "Logbook" to great effect.
Finally, we want to avoid overly onerous process obsession, so use of the logbook will be encouraged but not mandatory or policed.

## Decision

- We will create a "Logbook" directory intended to capture these kinds of informal notes.
- It will have a `log.md` file with a series of entries headings with the date in descending order.
- Any developer is encouraged to add an entry to the team logbook tracking their progress, thoughts, experiments, successes, failures, etc.
- Meetings among the core maintainers should also result in an entry added to the logbook if anything of substance is discussed.
- Additionally, the logbook can become an unofficial interconnected knowledge base; If you'd like to save some useful command, benchmarking result, musings on some experimental approach, etc. you may create a dedicated markdown file for it in the logbook directory.
- Engineers are encouraged to interlink these files where appropriate, using markdown formatted links.
- Since the directory consists of just markdown files (and supporting images), no extra tooling is needed, however we highlight [Obsidian](https://obsidian.md/) as a great tool for navigating and editing markdown directories like this.
- No official organization is yet imposed, but as the project continues to grow, we expect to revisit the organization of the logbook if it becomes unwieldy. The goal is to keep the process loose until it becomes needed.

## Consequence

- As a consequence, developers will have a place to record things that wouldn't make sense as official engineering decisions, and won't get lost in inaccessible discord chat history.
- Additionally, new developers to the project, or developers that have spent some time away from the project, will have a repository of knowledge to get quickly reacquainted with the progress of the project.

## Discussion Points
