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
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use amaru::{
    lifecycle::{Runnable, RuntimeKind},
    version,
};
use clap::Parser;
use clap_complete::{Shell, generate};

use crate::cli;

#[derive(Debug, Parser)]
pub struct Args {
    #[arg(long)]
    output_dir: PathBuf,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = args.output_dir;

    create_dir(output_dir.join("share/man/man1"))?;
    render_man_page(output_dir.as_path(), version::package_version())?;

    match version::target_os() {
        "windows" => render_windows_completion(output_dir.as_path(), version::package_version())?,
        _ => render_unix_completions(output_dir.as_path(), version::package_version())?,
    }

    Ok(())
}

fn create_dir(path: PathBuf) -> io::Result<()> {
    fs::create_dir_all(path)
}

fn render_man_page(output_dir: &Path, version: &'static str) -> Result<(), Box<dyn std::error::Error>> {
    let command = cli::command(version);
    let man = clap_mangen::Man::new(command);
    let path = output_dir.join("share/man/man1/amaru.1");
    let mut file = File::create(path)?;
    man.render(&mut file)?;
    Ok(())
}

fn render_completion(
    output_dir: &Path,
    version: &'static str,
    shell: Shell,
    relative_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = cli::command(version);
    let path = output_dir.join(relative_path);
    let mut file = File::create(path)?;
    generate(shell, &mut command, "amaru", &mut file);
    Ok(())
}

fn render_unix_completions(output_dir: &Path, version: &'static str) -> Result<(), Box<dyn std::error::Error>> {
    create_dir(output_dir.join("share/bash-completion/completions"))?;
    create_dir(output_dir.join("share/zsh/site-functions"))?;
    create_dir(output_dir.join("share/fish/vendor_completions.d"))?;

    render_completion(output_dir, version, Shell::Bash, Path::new("share/bash-completion/completions/amaru"))?;
    render_completion(output_dir, version, Shell::Zsh, Path::new("share/zsh/site-functions/_amaru"))?;
    render_completion(output_dir, version, Shell::Fish, Path::new("share/fish/vendor_completions.d/amaru.fish"))?;

    Ok(())
}

fn render_windows_completion(output_dir: &Path, version: &'static str) -> Result<(), Box<dyn std::error::Error>> {
    create_dir(output_dir.join("share/powershell/completions"))?;
    render_completion(output_dir, version, Shell::PowerShell, Path::new("share/powershell/completions/amaru.ps1"))
}
