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
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use quote::ToTokens;
use syn::Item;

use crate::{emit_rerun_if_exists, git, write_if_changed};

/// Generate `dump_schemas_type_aliases.rs` in `OUT_DIR`, containing a `TYPE_ALIASES` constant
/// that maps every top-level type alias of the workspace to its underlying type.
pub(crate) fn write_type_aliases_file() -> Result<()> {
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

    write_if_changed(&output, &contents)?;
    Ok(())
}

/// Collect the type aliases of every crate under `crates_dir`, preferably from the files
/// listed by git, otherwise by walking the directory tree.
fn collect_workspace_type_aliases(crates_dir: &Path, aliases: &mut BTreeMap<String, String>) -> Result<()> {
    if collect_git_workspace_type_aliases(crates_dir, aliases)? {
        return Ok(());
    }

    emit_rerun_if_exists(crates_dir);

    let entries = fs::read_dir(crates_dir)?;

    for entry in entries {
        let entry = entry?;
        let entry_path = entry.path();
        let file_type = entry.file_type()?;

        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }

        let crate_name = crate_ident(&entry_path);
        collect_type_aliases(&entry_path, aliases, crate_name.as_deref())?;
    }

    Ok(())
}

/// Collect the type aliases from the rust files listed by git, so that ignored files are
/// skipped. Return `false` when git is unavailable and a directory walk must be used instead.
fn collect_git_workspace_type_aliases(crates_dir: &Path, aliases: &mut BTreeMap<String, String>) -> Result<bool> {
    let Some(workspace_dir) = crates_dir.parent() else {
        return Ok(false);
    };

    let Ok(relative_paths) = git::get_workspace_rust_files(workspace_dir) else {
        return Ok(false);
    };

    for relative_path in relative_paths {
        let path = workspace_dir.join(&relative_path);
        if !path.symlink_metadata().is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }

        let crate_name = crate_name_from_workspace_relative_path(&relative_path);

        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path)?;
        collect_type_aliases_from_source(&source, aliases, crate_name.as_deref());
    }

    Ok(true)
}

/// Extract the crate name from a `crates/<crate-name>/...` path, normalized as an identifier.
fn crate_name_from_workspace_relative_path(path: &Path) -> Option<String> {
    let mut components = path.components();
    (components.next()?.as_os_str() == "crates")
        .then(|| components.next())
        .flatten()
        .and_then(|component| component.as_os_str().to_str())
        .map(|name| name.replace('-', "_"))
}

/// Collect the type aliases from every rust file under `path`, recursively.
fn collect_type_aliases(path: &Path, aliases: &mut BTreeMap<String, String>, crate_name: Option<&str>) -> Result<()> {
    let entries = fs::read_dir(path)?;

    for entry in entries {
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
            println!("cargo:rerun-if-changed={}", entry_path.display());
            let source = fs::read_to_string(&entry_path)?;
            collect_type_aliases_from_source(&source, aliases, crate_name);
        }
    }

    Ok(())
}

/// Parse a rust source file and record its top-level type aliases, both bare and
/// qualified with the crate name.
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

/// Normalize a crate directory name as a crate identifier (`amaru-kernel` -> `amaru_kernel`).
fn crate_ident(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(|name| name.replace('-', "_"))
}

/// Check the `.rs` extension.
fn is_rust_source_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("rs")
}

/// Return the name and normalized target type of a `type Alias = Target;` item,
/// skipping generic aliases whose parameters cannot be resolved by name alone.
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

/// Strip the whitespace introduced by token-stream printing (`Vec < u8 >` -> `Vec<u8>`).
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

    #[test]
    fn test_crate_name_from_workspace_relative_path_normalizes_hyphens() {
        assert_eq!(
            crate_name_from_workspace_relative_path(Path::new("crates/amaru-kernel/src/lib.rs")),
            Some("amaru_kernel".to_string())
        );
        assert_eq!(crate_name_from_workspace_relative_path(Path::new("README.md")), None);
    }
}
