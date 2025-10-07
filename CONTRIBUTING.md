# What & How Can You Contribute?

## Code

We are still early code-wise so it is a bit hard to provide any guidance here.
We will adjust and amend these instructions as soon as we feel ready to welcome
contributions from that side.

### Git Hook
We provide git hooks to lint code before it gets checked in CI/CD, simplfying the contribution process. To setup the hooks, please run:

```bash
./scripts/setup-hooks.sh
```

### Rust nightly

`amaru` currently relies on nightly versions of rust to benefit from following `features`:

* [try_trait_v2](https://github.com/rust-lang/rust/issues/84277)
* [assert_matches](https://github.com/rust-lang/rust/issues/82775)
* [step_trait](https://github.com/rust-lang/rust/issues/42168)

## Using radicle

While most of the activity on Amaru happens on [GitHub](https://github.com/pragma-org/amaru) is also compatible with Radicle, a decentralized code collaboration platform. This allows developers to collaborate on Amaru's development in a more decentralized manner.

For detailed instructions on how to install and use Radicle, please check the [user guide](https://radicle.xyz/guides/user).

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

## Other

There are various peripheral activities that are useful and welcome
contributions. If you're willing to contribute but don't know what and where to
start, here's a non-exhaustive list of ideas:

1. Use the software! Build projects and products with it, and tell us about it.
   In particular, <u><strong>if you are a stake pool operator</strong></u>, please reach out to be
   involved in the upcoming testnet and further development updates.

1. Identify and report defects. Notice anything that seems off? Let us know.

1. Propose features and ideas, backed by use-cases and examples.[^1]

1. Attend demos, delivery events and project pulse happening at a regular cadence.

1. Write tutorials, guides and/or record educational videos to help others with the project.

1. Proof-read and review documents for technical accuracy and understanding.

1. Willing to experiment? We are ready to mentor you to tackle one of the many available [ideas](https://github.com/pragma-org/amaru/discussions/categories/ideas?discussions_q=is%3Aopen+category%3AIdeas).

[^1]: A good feature request should mention a use-cases and a few personas, as well as the context in which that features is envisioned. Note that we are more interested in _problems_ than _solutions_. Often, a single solution for one group may not be ideal for another. But understanding what problem each group is trying to solve is crucial for designing the right solution.

# Need Help Getting Started?

Should you be unsure about where to start, feel free to [come and chat on Discord](https://discord.gg/3nZYCHW9Ns).
