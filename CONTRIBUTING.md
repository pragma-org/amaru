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

### Logbook

We keep an asynchronous public trail of progress in the form of a [LogBook](https://github.com/pragma-org/amaru/wiki/log) where each regular contributor is encouraged to contribute and share progress, challenges, thoughts and ideas via the logbook (see below).

Contributors who are actively working under a budget from the Cardano treasury must post weekly digests in the logbook answering:

- [ ] What has been achieved during the week?
- [ ] Which budget item or milestones does it help moving forward?
- [ ] What are the next steps?

> [!TIP]
> The first question can be answered by itemizing pull requests, no prose is needed.

Monthly summaries and labels are extracted from the logbook to facilitate its discovery.

### Treasury

Details about Amaru's treasury management are logged separately on a [financial journal](https://github.com/pragma-org/amaru-treasury/tree/main/journal#journal), maintained by the ongoing maintainers committee.

## Workflow

### GitHub Projects and Milestones

Work is tracked on our [GitHub Projects board](https://github.com/orgs/pragma-org/projects/3) with a simple **Todo** / **In Progress** / **Interrupted** / **Done** Kanban workflow.

Issues follow a [well-defined template](./.github/ISSUE_TEMPLATE/task.md).

Issues must be fully specified, including test strategy and documentation plan.

Those still under consideration or incomplete must live as [Github Discussions](https://github.com/pragma-org/amaru/discussions) to be refined and scoped.

Every issue must be assigned to a GitHub milestone that maps directly to a budget item.

### Living CHANGELOG

Any PRs that add, remove, modify or fix any *user-facing behavior* must include a [CHANGELOG](./CHANGELOG.md) entry — short and factual. Automated/LLM-generated suggestions are welcome, but humans must verify wording.

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

For anything regarding building, testing or running Amaru, please refer to the [README](./README.md) and [Makefile](./Makefile).

#### Rust nightly

We currently rely on nightly versions of rust to benefit from following `features`:

* [try_trait_v2](https://github.com/rust-lang/rust/issues/84277)
* [assert_matches](https://github.com/rust-lang/rust/issues/82775)
* [step_trait](https://github.com/rust-lang/rust/issues/42168)

#### Continuous Integration & git hooks

We have a rather sophisticated [continuous integration pipeline](./.github/workflows/continuous-integration.yml). Changes to the code will be expected to satisfy this pipeline.

We provide git hooks to lint code before it gets checked in CI/CD, simplifying the contribution process. To setup the hooks, please run:

```bash
./scripts/setup-hooks.sh
```

#### Continuous Delivery

We use `main` as a default branch. The latest version of `main` captures the latest version of the project.

It is therefore expected that `main` is kept in a working state *at all time*.

Pre-compiled executables are [continuously delivered](https://pragma-org.github.io/amaru/) upon changes to `main`.

In addition, we also provide [docker images](https://github.com/pragma-org/amaru/pkgs/container/amaru) continuously.

### Commits

We use [conventional commits](https://www.conventionalcommits.org/en/v1.0.0/) and all commits must be [gpg-signed](https://git-scm.com/book/ms/v2/Git-Tools-Signing-Your-Work).

Commits are expected to *at least compile*, to ease troubleshooting and bisecting in case of issues.

Avoid commits such as `tmp`, `wip` or `lint` that contain patches that could simply be squashed onto others.

Ideally, commits should optimize for _removal and cherry-picking_, so that it's easy to split work and troubleshoot bugs.

### Developer Certificate of Origin (DCO)

We respect intellectual property rights of others and we want to make sure all incoming contributions are correctly attributed and licensed. A [Developer Certificate of Origin (DCO)](https://developercertificate.org/) is a lightweight mechanism to do that.

So in addition, every commit must be signed off by its author(s), indicating an agreement to contribute under the project’s license and terms.

To sign off, use the [`--sign-off / -s`](https://git-scm.com/docs/git-commit#Documentation/git-commit.txt---signoff) git commit option.

### Radicle

While most of the activity on Amaru happens on [GitHub](https://github.com/pragma-org/amaru) is also compatible with Radicle, a decentralized code collaboration platform. This allows developers to collaborate on Amaru's development in a more decentralized manner.

For detailed instructions on how to install and use Radicle, please check the [user guide](https://radicle.xyz/guides/user).

<details><summary><strong>Using radicle</strong></summary>

Once radicle is installed on your machine, create a local clone of the Amaru repository with:

```bash
rad clone rad:zkw8cuTp2YRsk1U68HJ9sigHYsTu
```

If you want to contribute as a seeder, inside the repository do:

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

The maintainers committee is responsible for overseeing budgets, and their associated scopes of work.

They provide guidance on the roadmap and are responsible for ensuring the delivery of the budget items.

In particular, they have authority to manage the [Amaru committers](https://github.com/orgs/pragma-org/teams/amaru-committers) GitHub team.

### Merge / Review Authority

Changes to `main` are proposed via pull requests, and guarded by a [continuous integration pipeline](./.github/workflows/continuous-integration.yml), whose checks are expected to pass before any change is merged.

Contributors part of the [Amaru committers](https://github.com/orgs/pragma-org/teams/amaru-committers) GitHub team are expected to seek review from one or more other Amaru committer. Yet each member of this team is entrusted with its ability to make judgment on the mergeability of their changes and shall use that power with great care.

External contributors must have their contributions approved by at **least two members** of the [Amaru committers](https://github.com/orgs/pragma-org/teams/amaru-committers).

Occasionally, members of the [Amaru Maintainers Committee](https://github.com/orgs/pragma-org/teams/amaru-maintainers-committee) may request additional reviews or inputs from any contribution.

### Priority and Triage

Triage of [issues](https://github.com/pragma-org/amaru/issues) and [discussions](https://github.com/pragma-org/amaru/discussions) is performed synchronously at least once
per month, in a public call held on [PRAGMA's Discord](https://discord.gg/P7xUTjZxy6).

Items are prioritized based on impact, risk, and effort.

### Engineering Decision Records (EDRs)

Technical proposals with broad or long-term impact require an approved Engineering Decision Record (abbrev. EDRs). In general, an EDR is required when one of the following criteria is met:

- [ ] Affects external behavior in significant ways (e.g. break user-facing interfaces)
- [ ] Changes may cause important disruptions affecting the work of many contributors
- [ ] Changes testing/conformance strategy, or pauses a risk to software correctness
- [ ] Affects performances in a significant way
- [ ] Introduces a new dependency to a service, software and/or a substantially large library
- [ ] Alters governance or process

Each EDR is numbered, located under [./engineering-decision-records](./engineering-decision-records) and must follow [a well-defined template](./.github/templates/edr.md).
