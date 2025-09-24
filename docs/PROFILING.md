run inside devcontainer via vscode

# Heapstack

[Heapstack](https://github.com/KDE/heaptrack) is a heap memory profiler for Linux.

## OSx

Being Linux only, some steps are necessary to ensure heaptrack can be used on OSx.

### Pre requisites

[Devcontainer](https://containers.dev/) is leveraged to have amaru run in a pre-configured linux environment. Then [XQuartz](https://www.xquartz.org/) is used to display graphics directly on OSx host.

First install XQuartz

```
brew install --cask xquartz
```

then run it `open -a XQuartz` and allow remote external connections in menu `Settings/Security` then restart.

Finally, from within XQuartz integrated terminal, run `xhost +localhost`. *IMPORTANT* this has to be done each time XQuartz is restarted.

### Using VSCode

VSCode natively supports devcontainer so simply reopen your workspace in devcontainer. You will then get access to a regular VSCode instance as if it was running from within the docker instance.

#### Generate a heaptrack snapshot

```shell
cargo build --profile profiling
heaptrack ./target/profiling/amaru $YOUR_OPTIONS
```

#### Analyse heaptrack snapshot

```shell
heaptrack_gui SNAPSHOT_FILE
```

### Using another editor

Use [Dev Container CLI](https://github.com/devcontainers/cli) to automate interactions with the docker image:

```shell
devcontainer up
devcontainer exec --workspace-folder . cargo build --profile profiling
devcontainer exec --workspace-folder . heaptrack ./target/profiling/amaru
```
