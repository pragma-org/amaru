# CLI Tests

This directory is for testing the CLI of Amaru from the outside.

As it mainly consists of running shell commands and checking the outcome, we
use [bash_unit](https://github.com/bash-unit/bash_unit) for that.

## How to run the test

Ensure you have bash_unit installed. See
[how to install bash_unit](https://github.com/bash-unit/bash_unit?tab=readme-ov-file#how-to-install-bash_unit).

Run the following command:

```
#> bash_unit cli-tests/test_*
```

## How are the tests structured

We often need some sort of setup specific to the kind of checks we want to make
on the CLI. The `fixtures` subdirectory contains such setups and the tests `cd`
into the appropriate directory depending on the test needs.