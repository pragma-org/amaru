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
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use quote::ToTokens;
use syn::Item;

pub fn load_workspace_type_aliases_from(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let workspace_dir = find_workspace_dir(path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "failed to locate workspace root from current path"))?;

    collect_workspace_type_aliases(&workspace_dir.join("crates"))
}

pub fn resolve_type_alias(rust_type: &str, aliases: &BTreeMap<String, String>) -> String {
    let normalized = normalize_type_string(rust_type);
    let mut current = normalized.clone();
    let mut visited = BTreeSet::from([normalized]);

    while let Some(next) = aliases.get(&current) {
        if !visited.insert(next.clone()) {
            break;
        }

        current = next.clone();
    }

    current
}

fn collect_workspace_type_aliases(crates_dir: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut aliases = BTreeMap::new();

    for entry in fs::read_dir(crates_dir)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_type = entry.file_type()?;

        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }

        let crate_name = crate_ident(&entry_path);
        collect_type_aliases(&entry_path, &mut aliases, crate_name.as_deref())?;
    }

    Ok(aliases)
}

fn collect_type_aliases(
    path: &Path,
    aliases: &mut BTreeMap<String, String>,
    crate_name: Option<&str>,
) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            collect_type_aliases(&entry_path, aliases, crate_name)?;
            continue;
        }

        if file_type.is_file() && is_rust_source_file(&entry_path) {
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

fn find_workspace_dir(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.join("Cargo.toml").is_file() && ancestor.join("crates").is_dir())
        .map(Path::to_path_buf)
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
    fn test_collect_type_aliases_from_source_ignores_associated_types() {
        let mut aliases = BTreeMap::new();

        collect_type_aliases_from_source(
            r#"
            pub type Lovelace = u64;

            impl Deref for Coin {
                type Target = InnerCoin;
            }
            "#,
            &mut aliases,
            None,
        );

        assert_eq!(aliases, BTreeMap::from([("Lovelace".to_string(), "u64".to_string())]));
    }

    #[test]
    fn test_crate_ident_normalizes_hyphens() {
        assert_eq!(crate_ident(Path::new("amaru-kernel")), Some("amaru_kernel".to_string()));
    }

    #[test]
    fn test_parse_top_level_type_alias() {
        let item: Item = syn::parse_str("pub type Lovelace = u64;").unwrap();
        assert_eq!(parse_top_level_type_alias(&item), Some(("Lovelace".to_string(), "u64".to_string())));

        let generic_item: Item = syn::parse_str("pub type Wrapped<T> = Vec<T>;").unwrap();
        assert_eq!(parse_top_level_type_alias(&generic_item), None);
    }

    #[test]
    fn test_resolve_type_alias_resolves_transitively() {
        let aliases = BTreeMap::from([
            ("Amount".to_string(), "Lovelace".to_string()),
            ("Lovelace".to_string(), "u64".to_string()),
            ("amaru_kernel::Lovelace".to_string(), "u64".to_string()),
        ]);

        assert_eq!(resolve_type_alias("Amount", &aliases), "u64");
        assert_eq!(resolve_type_alias("amaru_kernel::Lovelace", &aliases), "u64");
    }

    #[test]
    fn test_resolve_type_alias_stops_on_cycles() {
        let aliases = BTreeMap::from([
            ("Amount".to_string(), "Lovelace".to_string()),
            ("Lovelace".to_string(), "Amount".to_string()),
        ]);

        assert_eq!(resolve_type_alias("Amount", &aliases), "Lovelace");
    }
}
