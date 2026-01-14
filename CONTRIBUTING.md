# Contributing to Amaru

Welcome! Thank you for your interest in contributing to **Amaru** — a community-driven, open-source implementation of Cardano.

This document explains how to contribute, our process and our governance rules in a way that helps maintain quality, predictability, and alignment with our community priorities.

> [!NOTE]
>
> If this feels overwhelming and you are unsure about where to start, feel free to [come and chat on Discord](https://discord.gg/3nZYCHW9Ns).

## Communication

### Discord

[PRAGMA's Discord](https://discord.gg/P7xUTjZxy6) is our main day-to-day communication channel.

Important decisions must be captured in an EDR or issues — Discord posts alone are not durable.

### Bi-weekly Meetings

We hold a bi-weekly meeting every other Tuesday, at 3 P.M. UTC.

This meeting serves for the [Priority and Triage](#priority-and-triage) of work items.

Though speaker time is mostly reserved to Amaru contributors, the meeting is public and open to all listeners.

### Logbook

We keep an asynchronous public trail of progress in the form of a [LogBook](https://github.com/pragma-org/amaru/wiki/log) where each regular contributor is encouraged to contribute and share progress, challenges, thoughts and ideas via the logbook.

Updates in the logbook may be frequent or occasional. However, contributors who are actively working under a budget from the Cardano treasury must post (at least) weekly digests in the logbook, answering:

- [ ] What has been achieved during the week?
- [ ] Which budget item or milestones does it help move forward?
- [ ] What are the next steps?

> [!TIP]
> The first question can be answered by itemising pull requests; no prose is needed.

Monthly summaries and labels are extracted from the logbook to facilitate its discovery.

### Treasury

Details about Amaru's treasury management are logged separately on a [financial journal](https://github.com/pragma-org/amaru-treasury/tree/main/journal#journal), maintained by the ongoing maintainers committee.

## Workflow

### GitHub Projects and Milestones

Work is tracked on our [GitHub Projects board](https://github.com/orgs/pragma-org/projects/3) with a simple **Todo** / **In Progress** / **Interrupted** / **Ready For Demo** / **Done** Kanban workflow.

Issues follow a [well-defined template](./.github/ISSUE_TEMPLATE/task.yml).

Issues must be fully specified, including test strategy and documentation plan.

Those still under consideration or incomplete must live as [GitHub Discussions](https://github.com/pragma-org/amaru/discussions) to be refined and scoped.

Every issue must be assigned to a GitHub milestone that maps directly to a budget item.

### Living CHANGELOG

Any PRs that add, remove, modify or fix any *user-facing behaviour* must include a [CHANGELOG](./CHANGELOG.md) entry — short and factual. Automated/LLM-generated suggestions are welcome, but humans must verify wording.

A user-facing behaviour is anything that a user of Amaru (as an executable or as a collection of libraries) may seemingly notice. This includes (but isn't limited to):

- command-line commands, options and arguments
- default behaviours and default configurations
- any exposed protocol API
- supported compilation targets
- exposed logs, traces and metrics
- `pub` modules or functions
- databases formats
- performances

> [!TIP]
> We follow specific guidelines to [keep a changelog](https://keepachangelog.com/en/1.0.0/):
>
> #### Guiding principles
>
> - Changelogs are for humans, not machines.
> - There should be an entry for every single version.
> - The same types of changes should be grouped.
> - Versions and sections should be linkable.
> - The latest version comes first.
> - The release date of each version is displayed.
>
> #### Types of changes
>
> - `Added` for new features.
> - `Changed` for changes in existing functionality.
> - `Deprecated` for soon-to-be removed features.
> - `Removed` for now removed features.
> - `Fixed` for any bug fixes.
> - `Security` in case of vulnerabilities.

### Coding Practices

#### Basics

We expect contributors to use Rust's compiler, [clippy](https://doc.rust-lang.org/clippy/) and [formatter](https://github.com/rust-lang/rustfmt) according to the toolchain specified under [./rust-toolchain.toml] and configuration found in [.cargo](./.cargo).

For anything related to building, testing or running Amaru, please refer to the [README](./README.md) and [Makefile](./Makefile).

#### Rust nightly

We sometimes rely on nightly features of Rust to benefit from specific features.

The use of Rust nightly features must first be discussed and approved as an Engineering Decision Record.

When such specific features are turned on, they shall come with an accompanying comment pointing at an approved

#### Continuous Integration & Git hooks

We have a rather sophisticated [continuous integration pipeline](./.github/workflows/continuous-integration.yml). Changes to the code must satisfy this pipeline.

We provide Git hooks to lint code before it's checked in CI/CD, simplifying the contribution process. To set up the hooks, please run:

```bash
./scripts/setup-hooks.sh
```

#### Continuous Delivery

We use `main` as a default branch. The latest version of `main` captures the latest version of the project.

It is therefore expected that `main` is kept in a _working state_ (compiles + all tests & CI checks pass) *at all times*.

Pre-compiled executables are [continuously delivered](https://pragma-org.github.io/amaru/) upon changes to `main`.

In addition, we also provide [docker images](https://github.com/pragma-org/amaru/pkgs/container/amaru) continuously.

#### Use of AI

We recognize the existence and importance of (generative) AI as part of our workflow for it has proven to be a useful **tool**.

We use it for reviews, for domain exploration, as search engine or for generating code when we see fit.

Our rule of thumb is: do not generate or publish anything you wouldn't feel confident writing yourself.

Importantly, AI cannot substitute itself for human validation and any use of _unsupervised AI_ is prohibited.

### Commits

We use [conventional commits](https://www.conventionalcommits.org/en/v1.0.0/), and all commits must be [gpg-signed](https://git-scm.com/book/ms/v2/Git-Tools-Signing-Your-Work).

Commits must *at least compile*, to ease troubleshooting and bisecting in case of issues.

Avoid commits such as `tmp`, `wip` or `lint` that contain patches that could be squashed into others.

Ideally, commits should optimise for _removal and cherry-picking_, so that it's easy to split work and troubleshoot bugs.

### Developer Certificate of Origin (DCO)

We respect the others' intellectual property rights and want to ensure all incoming contributions are correctly attributed and licensed. A [Developer Certificate of Origin (DCO)](https://developercertificate.org/) is a lightweight mechanism to do that.

In addition, every commit must be signed off by its author(s), indicating agreement to contribute under the project’s license and terms.

To sign off, use the [`--sign-off / -s`](https://git-scm.com/docs/git-commit#Documentation/git-commit.txt---signoff) git commit option.

### Radicle

While most of the activity on Amaru happens on [GitHub](https://github.com/pragma-org/amaru) is also compatible with Radicle, a decentralised code collaboration platform. This allows developers to collaborate on Amaru's development in a more decentralised manner.

For detailed instructions on how to install and use Radicle, please check the [user guide](https://radicle.xyz/guides/user).

<details><summary><strong>Using Radicle</strong></summary>

Once Radicle is installed on your machine, create a local clone of the Amaru repository with:

```bash
rad clone rad:zkw8cuTp2YRsk1U68HJ9sigHYsTu
```

If you want to contribute as a seeder, inside the repository, do:

```bash
$ rad seed
╭───────────────────────────────────────────────────────────╮
│ Repository                         Name    Policy   Scope │
├───────────────────────────────────────────────────────────┤
│ rad:zkw8cuTp2YRsk1U68HJ9sigHYsTu   amaru   allow    all   │
╰───────────────────────────────────────────────────────────╯
```

To propagate changes as patches:

```bash
git push rad HEAD:refs/patches
```

</details>

## Governance

### Maintainers Committee

A [maintainers committee](./engineering-decision-records/018-amaru-maintainers-committee.md) is established by the PRAGMA-appointed lead maintainer: [KtorZ](https://github.com/KtorZ).

The maintainers committee is responsible for overseeing budgets and their associated scopes of work.

They provide guidance on the roadmap and are responsible for ensuring the delivery of the budget items.

In particular, they have the authority to manage the [Amaru committers](https://github.com/orgs/pragma-org/teams/amaru-committers) GitHub team.

### Merge / Review Authority

Changes to `main` are proposed via pull requests, and guarded by a [continuous integration pipeline](./.github/workflows/continuous-integration.yml), whose checks must pass before any change is merged.

Contributors part of the [Amaru committers](https://github.com/orgs/pragma-org/teams/amaru-committers) GitHub team shall seek review from one or more other Amaru committers. Yet each member of this team is entrusted with the ability to make judgment on the mergeability of their changes and shall use that power with great care.

External contributors must have their contributions approved by at **least two members** of the [Amaru committers](https://github.com/orgs/pragma-org/teams/amaru-committers).

Occasionally, members of the [Amaru Maintainers Committee](https://github.com/orgs/pragma-org/teams/amaru-maintainers-committee) may request additional reviews or inputs from any contribution.

### Priority and Triage

Triage of [issues](https://github.com/pragma-org/amaru/issues) and [discussions](https://github.com/pragma-org/amaru/discussions) is performed:

- synchronously
- at least once per month
- in a public recurring call held on [PRAGMA's Discord](https://discord.gg/P7xUTjZxy6)
- under 2h each time

Participants to that call priorise items based on **impact**, **risk**, and **effort**. Whoever is present are the right people.

Meetings shall be structured as such:

1. Priorisation and/or promotion of discussions
2. Priorisation and assignation of issues
3. Occasional high-level reviews of important PRs
4. Open mic, if time allows

Issues shall not be assigned to people not participating and/or that haven't explicitly given their consent.

### Engineering Decision Records (EDRs)

Technical proposals with broad or long-term impact require an approved Engineering Decision Record (abbrev. EDRs). In general, an EDR is required when one of the following criteria is met:

- [ ] Affects external behaviour in significant ways (e.g. breaks user-facing interfaces)
- [ ] Changes may cause important disruptions affecting the work of many contributors
- [ ] Changes testing/conformance strategy, or poses a risk to software correctness
- [ ] Affects performances in a significant way
- [ ] Introduces a new dependency to a service, software and/or a substantially large library
- [ ] Alters governance or process

Each EDR is numbered, located under [./engineering-decision-records](./engineering-decision-records) and must follow [a well-defined template](./.github/templates/edr.md).

EDRs must be approved by the lead maintainer and at least one other [Amaru committer](https://github.com/orgs/pragma-org/teams/amaru-committers).
