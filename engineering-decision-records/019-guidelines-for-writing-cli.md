---
type: architecture
status: accepted
---

# Guidelines For Writing Command-Line Interfaces

## Context

<!-- Provide context, explain where the decision came from and why it's necessary to make one. -->

Command-line interfaces (abbrev. CLIs) are first-class in our world. Most of our users will primarily interact with Amaru via the means of a CLI.

Consistency is key to build good interface; and it is hard to get consistency unless guidelines and conventions are clearly spelled out.

This EDRs defines guidelines to follow when building CLI in the context of the Amaru project to keep the CLIs tidy and maximise the user experience.

## Decision

<!-- Explain the decision with sufficient details to be self-explanatory. -->

<details><summary><strong>1. Options shall have associated environment variables as fallback.</strong></summary>

Many users enjoy using env vars for configuring their deployments.

However, environment variables are hard to document. Command-line options, on
the contrary, are easy to document. So while we expect env vars to be the main
mechanism through which users configure Amaru's commands, we tie them to
options for easy discoverability.

An exception to this are _destructive options_ (e.g. `--force`) which must
always be passed explicitly to avoid catastrophic situations.
</details>

<details><summary><strong>2. Use a common prefix for envionment variables (e.g. <code>AMARU_</code>).</strong></summary>

Amaru nor Cardano aren't (yet?) an industry standard. Many names used across
options are ambiguous and may conflict with other tools used in the same
environment. Namespacing all variables makes clear at first glance what is
specific to Amaru and what isn't.
</details>

<details><summary><strong>3. Well-known/conventional env vars are exceptions to (2).</strong></summary>

We embrace "convention over configuration". If a convention already exists
(e.g. [Open Telemetry variables](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/))
widely used, we should not deviate from it.
</details>

<details><summary><strong>4. Log final options considered for the each command.</strong></summary>

It can be hard to figure out what exactly gets used as final values: we have
defaults, env var fallbacks, and arguments derived from other arguments.

Displaying a summary as an `INFO` level log event clarifies the execution steps and help user troubleshoot configuration issues down the line.

Note that, we follow the following conventions:

- One field per arg / option
- Ordered alphabetically
- A single `_command` field reinstate the command name
- An additional log message may follow.

```rust
info!(
    _command="run",
    chain_dir=%chain_dir.to_string_lossy(),
    ledger_dir=%ledger_dir.to_string_lossy(),
    listen_address=args.listen_address,
    max_downstream_peers = args.max_downstream_peers,
    max_extra_ledger_snapshots = %args.max_extra_ledger_snapshots,
    migrate_chain_db = args.migrate_chain_db,
    network=%args.network,
    peer_address=%args.peer_address.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
    pid_file=%args.pid_file.unwrap_or_default().to_string_lossy(),
    "running"
);
```
</details>

<details><summary><strong>5. Use consistent `value_names` and `env` var names.</strong></summary>

`value_names` (a.k.a. meta-variables) refer to the kind of data expected for
the underlying option (filepath, dir, tcp address, etc..). It should be as
informative as possible, and identical for options that refer to objects of the
same nature.
</details>

<details><summary><strong>6. Provide sound defaults whenever possible</strong></summary>

It's easy to get lost in _too many options_. So as much as possible, commands
should run with as little configuration as possible. In particular:

- Defaults must all be consistent with one-another between commands (!);
- Defaults may be derived from other options when necessary (we can derive a lot from the --network for example);
</details>

<details><summary><strong>7. Use args for mandatory values, and options for optional ones</strong></summary>

This isn't a golden rule, but a recommendation whenever possible. The downside
of using args is that they are nameless and positional. So it may rapidly get
out of hand once there are more than 2 kinds of arguments.

However, if (6) is followed carefully, the number of mandatory arguments shall
remain under control, and most elements moved to options.

The upside of arguments is that they can be documented separate from options,
and appear first. It's easy to overwhelm users with options, so having the
ability to separate the *actually user-required bits* is interesting.
</details>

<details><summary><strong>8. Use flags for boolean values</strong></summary>

Don't:

```rust
#[arg(
    short,
    long,
    value_name = "BOOL",
    default_value_t = false
)]
force: bool,

```

Do instead:

```rust
#[arg(
    short,
    long,
    action = ArgAction::SetTrue,
    default_value_t = false,
)]
force: bool,
```

It is shorter, more intuitive and easier to document.
</details>

<details><summary><strong>9. Document all options, with at least a top-level short description</strong></summary>

We use [clap](https://docs.rs/clap) as a command-line builder, which comes with
various conventions. One convention being that Rust doc comments on options are
translated into command-line descriptions.

Clap provides a short `-h` and long `--help` usage helps. In the short version,
only the first line of each description is shown. So it's important to
structure comments in this way too, ensuring that the important information
comes first; and details about the option comes after a single newline.
</details>

<details><summary><strong>10. Always provide long option names and avoid short/single-letter ones</strong></summary>

Words are cheap, whenever possible, use long names. They are more descriptive
and less error-prone. Short names shall only be used when they are very common
for specific kind of options (e.g. `--force / -f`, `--out` / `-o`).
</details>

<details><summary><strong>11. Display options in alphabetical order</strong></summary>

There's no "good manual order" for things like options. What one person would
think as logical, one other would deem messy. So the only real choice for
displaying options is to sort them alphabetically. It follows a principle of
least surprise and when done consistently, is easy to spot.
</details>


## Consequences

<!-- Describe the result/consequences of applying that decision; both positive and negative outcomes -->

- To ensure a consistent `value_names` and `env` across all commands, we have factored them into dedicated modules under [amaru's lib](https://github.com/pragma-org/amaru/blob/main/crates/amaru/src/lib.rs).
- Various command-line changes introduced via [amaru#636](https://github.com/pragma-org/amaru/pull/636) to comply with these guidelines.

## Discussion points

<!-- Summarizes, a posteriori, the major discussion points around that decisions -->

- [amaru#636](https://github.com/pragma-org/amaru/pulls/636)
