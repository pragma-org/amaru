// Copyright 2024 PRAGMA
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
    env, fs,
    path::{Path, PathBuf},
};

use quote::ToTokens;
use syn::Item;

#[expect(clippy::expect_used)]
fn main() {
    built::write_built_file().expect("Failed to acquire build-time information");
    write_type_aliases_file().expect("Failed to generate embedded type aliases for dump_schemas");

    // Set the AMARU_NETWORK environment variable based on the value from the environment or default to "preprod"
    // This is necessary for the tests to run correctly, as they rely on this variable to be set at build time
    let network = env::var("AMARU_NETWORK").unwrap_or_else(|_| "preprod".into());
    println!("cargo:rerun-if-env-changed=AMARU_NETWORK");
    println!("cargo:rustc-env=AMARU_NETWORK={}", network);
}

fn write_type_aliases_file() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let crates_dir = manifest_dir.join("../..").join("crates");
    let mut aliases = BTreeMap::new();

    collect_workspace_type_aliases(&crates_dir, &mut aliases)?;

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let output = out_dir.join("dump_schemas_type_aliases.rs");

    let contents = format!(
        "pub const TYPE_ALIASES: &[(&str, &str)] = &[\n{}\n];\n",
        aliases.iter().map(|(alias, target)| format!("    ({alias:?}, {target:?}),")).collect::<Vec<_>>().join("\n")
    );

    fs::write(output, contents)?;
    Ok(())
}

fn collect_workspace_type_aliases(
    crates_dir: &Path,
    aliases: &mut BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries = fs::read_dir(crates_dir)?;

    for entry in entries.flatten() {
        let entry_path = entry.path();

        if !entry_path.is_dir() {
            continue;
        }

        let crate_name = crate_ident(&entry_path);
        collect_type_aliases(&entry_path, aliases, crate_name.as_deref())?;
    }

    Ok(())
}

fn collect_type_aliases(
    path: &Path,
    aliases: &mut BTreeMap<String, String>,
    crate_name: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries = fs::read_dir(path)?;

    for entry in entries.flatten() {
        let entry_path = entry.path();

        if entry_path.is_dir() {
            collect_type_aliases(&entry_path, aliases, crate_name)?;
            continue;
        }

        if is_rust_source_file(&entry_path) {
            println!("cargo:rerun-if-changed={}", entry_path.display());
            let source = fs::read_to_string(&entry_path)?;
            collect_type_aliases_from_source(&source, aliases, crate_name);
        }
    }

    Ok(())
}

fn collect_type_aliases_from_source(source: &str, aliases: &mut BTreeMap<String, String>, crate_name: Option<&str>) {
    let Ok(syntax) = syn::parse_file(source) else {
        return;
    };

    syntax.items.iter().filter_map(parse_top_level_type_alias).for_each(|(alias, target)| {
        aliases.insert(alias.clone(), target.clone());
        if let Some(crate_name) = crate_name {
            aliases.insert(format!("{crate_name}::{alias}"), target);
        }
    });
}

fn crate_ident(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(|name| name.replace('-', "_"))
}

fn is_rust_source_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("rs")
}

fn parse_top_level_type_alias(item: &Item) -> Option<(String, String)> {
    let Item::Type(type_alias) = item else {
        return None;
    };

    if !type_alias.generics.params.is_empty() {
        return None;
    }

    let alias = type_alias.ident.to_string();
    let target = normalize_type_string(&type_alias.ty.to_token_stream().to_string());

    (!alias.is_empty() && !target.is_empty()).then_some((alias, target))
}

fn normalize_type_string(ty: &str) -> String {
    ty.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_type_aliases_from_source_adds_crate_qualified_aliases() {
        let mut aliases = BTreeMap::new();

        collect_type_aliases_from_source(
            r#"
            pub type Lovelace = u64;
            type Amount = Lovelace;
            "#,
            &mut aliases,
            Some("amaru_kernel"),
        );

        assert_eq!(aliases.get("Lovelace"), Some(&"u64".to_string()));
        assert_eq!(aliases.get("Amount"), Some(&"Lovelace".to_string()));
        assert_eq!(aliases.get("amaru_kernel::Lovelace"), Some(&"u64".to_string()));
        assert_eq!(aliases.get("amaru_kernel::Amount"), Some(&"Lovelace".to_string()));
    }

    #[test]
    fn test_crate_ident_normalizes_hyphens() {
        assert_eq!(crate_ident(Path::new("amaru-kernel")), Some("amaru_kernel".to_string()));
    }
}
