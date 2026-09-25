// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    collections::BTreeMap,
    io::{self, Write as _},
    str::FromStr,
};

use amaru::{
    lifecycle::{Runnable, RuntimeKind},
    value_names,
};
use anyhow::{Context, bail};
use clap::{Arg, ArgAction, Command, Parser};

use crate::{cli, version};

/// Generate an environment file from the node command-line interface.
#[derive(Debug, Parser)]
pub struct Args {
    /// Override the default value of an environment variable.
    #[arg(long = "override", value_name = value_names::STR_KEY_VALUE)]
    overrides: Vec<EnvironmentOverride>,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let mut variables = collect_environment_variables(&cli::command(version::display_version()))?;
    apply_overrides(&mut variables, args.overrides)?;
    let output = render_environment_variables(&variables);
    io::stdout().write_all(output.as_bytes()).context("failed to write environment configuration")?;
    Ok(())
}

fn render_environment_variables(variables: &BTreeMap<String, EnvironmentVariable>) -> String {
    let mut output = String::new();

    for (index, (name, variable)) in variables.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str("# <");
        output.push_str(&variable.meta_type);
        output.push_str("> ");
        output.push_str(&variable.description);
        output.push('\n');
        if variable.default_value.is_none() {
            output.push_str("# ");
        }
        output.push_str(name);
        output.push('=');
        if let Some(value) = variable.default_value.as_ref() {
            output.push_str(&if is_scalar(value) { value.to_string() } else { shell_quote(value) })
        }
        output.push('\n');
    }

    output
}

// -------------------------------------------------------------------------------------------------
// EnvironmentOverride
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct EnvironmentOverride {
    name: String,
    value: String,
}

impl FromStr for EnvironmentOverride {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((name, value)) = value.split_once('=') else {
            return Err(format!("expected {}", value_names::STR_KEY_VALUE));
        };

        if name.is_empty() || value.is_empty() {
            return Err("environment variable name or value cannot be empty".to_string());
        }

        Ok(Self { name: name.to_string(), value: value.to_string() })
    }
}

fn apply_overrides(
    variables: &mut BTreeMap<String, EnvironmentVariable>,
    overrides: Vec<EnvironmentOverride>,
) -> anyhow::Result<()> {
    for EnvironmentOverride { name, value } in overrides {
        let variable = variables
            .get_mut(&name)
            .with_context(|| format!("`{name}` is not an environment variable for `node run` or `node bootstrap`"))?;
        variable.default_value = Some(value);
    }

    Ok(())
}

// -------------------------------------------------------------------------------------------------
// EnvironmentVariable
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnvironmentVariable {
    description: String,
    default_value: Option<String>,
    meta_type: String,
}

fn collect_environment_variables(command: &Command) -> anyhow::Result<BTreeMap<String, EnvironmentVariable>> {
    let node = find_subcommand(command, "node")?;
    let run = find_subcommand(node, "run")?;
    let bootstrap = find_subcommand(node, "bootstrap")?;

    let mut variables = BTreeMap::new();

    extend_environment_variables(&mut variables, command)?;
    extend_environment_variables(&mut variables, run)?;
    extend_environment_variables(&mut variables, bootstrap)?;

    Ok(variables)
}

fn find_subcommand<'a>(command: &'a Command, name: &str) -> anyhow::Result<&'a Command> {
    command
        .get_subcommands()
        .find(|subcommand| subcommand.get_name() == name)
        .with_context(|| format!("missing `{name}` subcommand in the Clap definition"))
}

fn extend_environment_variables(
    variables: &mut BTreeMap<String, EnvironmentVariable>,
    command: &Command,
) -> anyhow::Result<()> {
    for (arg, env) in command.get_arguments().filter_map(|arg| arg.get_env().map(|env| (arg, env))) {
        let name = env.to_string_lossy().into_owned();
        if name.starts_with("AMARU_GLOBAL_") {
            continue;
        }
        let variable = EnvironmentVariable::from_arg(arg)?;

        match variables.entry(name) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(variable);
            }
            std::collections::btree_map::Entry::Occupied(entry)
                if entry.get().default_value == variable.default_value
                    && entry.get().meta_type == variable.meta_type => {}
            std::collections::btree_map::Entry::Occupied(entry) => {
                bail!("environment variable `{}` has conflicting Clap defaults or types", entry.key());
            }
        }
    }

    Ok(())
}

impl EnvironmentVariable {
    fn from_arg(arg: &Arg) -> anyhow::Result<Self> {
        let description = arg
            .get_help()
            .map(ToString::to_string)
            .and_then(|help| help.lines().next().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string))
            .map(into_sentence)
            .with_context(|| {
                format!("environment-backed option `--{}` is missing a description", arg.get_long().unwrap_or_default())
            })?;

        let default_value =
            arg.get_default_values().iter().map(|value| value.to_string_lossy()).collect::<Vec<_>>().join(",");
        let default_value = if default_value.is_empty() && matches!(arg.get_action(), ArgAction::SetTrue) {
            "false".to_string()
        } else if default_value.is_empty() && matches!(arg.get_action(), ArgAction::SetFalse) {
            "true".to_string()
        } else {
            default_value
        };

        let meta_type = if matches!(arg.get_action(), ArgAction::SetTrue | ArgAction::SetFalse) {
            "BOOL".to_string()
        } else {
            match arg.get_value_names() {
                Some(names) => names.iter().map(ToString::to_string).collect::<Vec<_>>().join(","),
                None => "STRING".to_string(),
            }
        };

        Ok(Self {
            description,
            default_value: if default_value.is_empty() { None } else { Some(default_value) },
            meta_type,
        })
    }
}

fn into_sentence(mut description: String) -> String {
    if !description.chars().last().is_some_and(|character| matches!(character, '.' | '!' | '?' | ';' | ':')) {
        description.push('.');
    }
    description
}

fn is_scalar(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_digit()) || ["true", "false"].contains(&value.to_lowercase().as_str())
}

fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\"');
    for character in value.chars() {
        if matches!(character, '\\' | '\"' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('\"');
    quoted
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use clap::{Arg, Command};

    use super::*;

    #[test]
    fn renders_sorted_environment_variables() {
        let command = Command::new("amaru")
            .arg(
                Arg::new("second")
                    .long("second")
                    .env("AMARU_SECOND")
                    .help("The second variable.")
                    .value_name("FILEPATH"),
            )
            .arg(
                Arg::new("first")
                    .long("first")
                    .env("AMARU_FIRST")
                    .help("The first variable.")
                    .action(ArgAction::SetTrue)
                    .default_value("false"),
            );

        let mut variables = BTreeMap::new();
        extend_environment_variables(&mut variables, &command).expect("arguments are valid");

        assert_eq!(
            render_environment_variables(&variables),
            "# <BOOL> The first variable.\nAMARU_FIRST=false\n\n# <FILEPATH> The second variable.\n# AMARU_SECOND=\n"
        );
    }

    #[test]
    fn applies_known_overrides() {
        let mut variables = BTreeMap::from([(
            "AMARU_NETWORK".to_string(),
            EnvironmentVariable {
                description: "The target network.".to_string(),
                default_value: None,
                meta_type: "NETWORK".to_string(),
            },
        )]);

        apply_overrides(
            &mut variables,
            vec![EnvironmentOverride { name: "AMARU_NETWORK".to_string(), value: "mainnet".to_string() }],
        )
        .expect("override should be applied");

        assert_eq!(variables["AMARU_NETWORK"].default_value.as_deref(), Some("mainnet"));
    }

    #[test]
    fn rejects_unknown_overrides() {
        let mut variables = BTreeMap::new();

        let err = apply_overrides(
            &mut variables,
            vec![EnvironmentOverride { name: "AMARU_UNKNOWN".to_string(), value: "value".to_string() }],
        )
        .expect_err("unknown variables must not be ignored");

        assert!(err.to_string().contains("AMARU_UNKNOWN"));
    }

    #[test]
    fn collects_the_node_environment_contract() {
        let variables = collect_environment_variables(&cli::command("test")).expect("command tree is valid");

        assert_eq!(variables["AMARU_WITH_JSON_TRACES"].meta_type, "BOOL");
        assert_eq!(variables["AMARU_LEDGER_MAX_EXTRA_SNAPSHOTS"].meta_type, "UINT|all");
        assert!(variables.contains_key("AMARU_S3_BUCKET"));
        assert!(!variables.contains_key("AMARU_GLOBAL_SYSTEM_START"));
        assert!(!variables.contains_key("AMARU_SNAPSHOTS_DIR"));
    }
}
