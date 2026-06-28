use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs::{File, create_dir_all, read_dir, remove_dir_all, remove_file, rename};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf, absolute};

use anyhow::{Result, anyhow, ensure};
use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Add { source, target } => add(source, target),
        Command::Mv { from, to } => mov(from, to),
        Command::Rm { target } => remove(target),
        Command::Check => check(),
        Command::Deploy { path, force } => deploy(path, force),
        Command::Init { path } => init(path),
    }
}

fn add(source: PathBuf, target: Option<PathBuf>) -> Result<()> {
    let mut cfg = Config::load()?;

    let source: PathBuf = source.components().collect();
    let src_filename =
        source.file_name().ok_or_else(|| anyhow!("path is invalid: {}", source.display()))?.into();
    let target_rel = target.unwrap_or(src_filename);
    ensure!(target_rel.is_relative(), "target must be relative: {}", target_rel.display());

    let target = cfg.root.join(target_rel);
    ensure!(!cfg.files.contains_key(&target), "config already contains file: {}", target.display());

    ensure!(source.exists(), "file does not exist: {}", source.display());
    ensure!(!target.exists(), "target already exists: {}", target.display());

    create_parent(&target)?;
    rename(&source, &target)?;
    force_symlink(&target, &source)?;

    cfg.files.insert(target, absolute(source)?);
    cfg.save()?;
    Ok(())
}

fn mov(from: PathBuf, to: PathBuf) -> Result<()> {
    let mut cfg = Config::load()?;

    ensure!(from.is_relative(), "paths must be relative: {}", from.display());
    ensure!(to.is_relative(), "paths must be relative: {}", to.display());
    let from = cfg.root.join(&from);
    let to = cfg.root.join(&to);
    ensure!(cfg.files.contains_key(&from), "config does not contain file: {}", from.display());
    ensure!(!cfg.files.contains_key(&to), "config already contains file: {}", to.display());
    ensure!(from.exists(), "file does not exist: {}", from.display());
    ensure!(!to.exists(), "file already exists: {}", to.display());

    rename(&from, &to)?;
    let source = cfg.files.remove(&from).unwrap();
    force_symlink(&to, &source)?;
    cfg.files.insert(to, source);
    cfg.save()?;
    Ok(())
}

fn remove(target: PathBuf) -> Result<()> {
    let mut cfg = Config::load()?;

    ensure!(target.is_relative(), "target must be relative: {}", target.display());
    let target = cfg.root.join(&target);
    ensure!(cfg.files.contains_key(&target), "config does not contain file: {}", target.display());
    ensure!(target.exists(), "file does not exist: {}", target.display());
    let original = cfg.files.remove(&target).unwrap();
    ensure!(
        original.read_link().is_ok_and(|p| p == target),
        "original location is not a link pointing to target: {}",
        original.display()
    );

    remove_file(&original)?;
    rename(target, original)?;
    cfg.save()?;
    Ok(())
}

fn check() -> Result<()> {
    if Config::load()?.check(false) {
        eprintln!("{} all good!", "Info:".green().bold());
    }
    Ok(())
}

fn deploy(root: Option<PathBuf>, force: bool) -> Result<()> {
    let root = root.unwrap_or(dotfile_path());
    ensure!(root.is_dir(), "path is not a directory");

    let cfg = Config::load_from(&root.join("confast/config.yaml"))?;
    ensure!(cfg.check(true), "please resolve these issues before continuing");

    for (target, source) in &cfg.files {
        if !source.exists() || force {
            create_parent(target)?;
            force_symlink(target, source)?;
        } else {
            eprintln!("Skipping existing file: {}", source.display());
        }
    }
    Ok(())
}

fn init(root: Option<PathBuf>) -> Result<()> {
    let root = root.and_then(|p| absolute(p).ok()).unwrap_or(dotfile_path());
    let managed_cfg_dir = root.join("confast");
    let managed_cfg_file = managed_cfg_dir.join("config.yaml");
    ensure!(
        !managed_cfg_file.exists(),
        "config already exists in target directory: {}",
        managed_cfg_file.display()
    );

    let cfg_dir = config_dir().join("confast");
    let cfg = Config {
        root,
        ignored: vec![".git".into(), ".gitignore".into()],
        files: [(managed_cfg_dir.clone(), cfg_dir.clone())].into(),
    };

    create_parent(&cfg_dir)?;
    create_parent(&managed_cfg_file)?;
    force_symlink(&managed_cfg_dir, &cfg_dir)?;
    cfg.save_to(&managed_cfg_file)?;
    Ok(())
}

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add a new managed file
    Add {
        /// Path to original file
        source: PathBuf,
        /// Path to managed file, relative to dotfiles directory
        target: Option<PathBuf>,
    },
    /// Move a managed file
    Mv {
        /// Relative to dotfiles directory
        from: PathBuf,
        /// Relative to dotfiles directory
        to: PathBuf,
    },
    /// Unmanage a file
    Rm {
        /// Relative to dotfiles directory
        target: PathBuf,
    },
    /// Check the config
    Check,
    /// Deploy dotfiles
    Deploy {
        /// Path to dotfiles directory, default is $HOME/.confast
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Overwrite existing files
        #[arg(short, long)]
        force: bool,
    },
    /// Initialize the program
    Init {
        /// Path to dotfiles directory, default is $HOME/.confast
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
struct Config {
    root: PathBuf,
    ignored: Vec<PathBuf>,
    // when actually stored:
    // keys (also the ignored list) are relative to dotfiles directory, and values are relative to home
    // this in intended to make them shorter so it'll be friendlier in case of manual editing
    files: BTreeMap<PathBuf, PathBuf>,
}

impl Config {
    fn load_from(path: &Path) -> Result<Self> {
        let cfg: Self = yaml_serde::from_reader(File::open(path)?)?;
        let home = home_dir();
        let ignored = cfg.ignored.into_iter().map(|p| cfg.root.join(p)).collect();
        let files = cfg.files.into_iter().map(|(t, s)| (cfg.root.join(t), home.join(s))).collect();
        Ok(Self { root: cfg.root, ignored, files })
    }

    fn load() -> Result<Self> {
        Self::load_from(&config_path())
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        let home = home_dir();
        let ignored =
            self.ignored.iter().map(|p| p.strip_prefix(&self.root).unwrap().to_owned()).collect();
        let files = self
            .files
            .iter()
            .map(|(target, source)| {
                let target = target.strip_prefix(&self.root).unwrap().to_owned();
                let source = source.strip_prefix(&home).unwrap().to_owned();
                (target, source)
            })
            .collect();
        let cfg = Self { root: self.root.clone(), ignored, files };
        yaml_serde::to_writer(File::create(path)?, &cfg)?;
        Ok(())
    }

    fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    fn check(&self, deploy_mode: bool) -> bool {
        let mut all_right = true;
        let managed_files: HashSet<_> = self.files.keys().collect();
        for (target, source) in &self.files {
            if !target.exists() {
                eprintln!("{} file does not exist: {}", "Error:".red().bold(), target.display());
                all_right = false
            }

            if deploy_mode {
                continue;
            }

            if !source.is_symlink() {
                eprintln!("{} source is not a link: {}", "Error:".red().bold(), source.display());
                all_right = false;
            } else {
                if !source.exists() {
                    eprintln!("{} link is broken: {}", "Error:".red().bold(), source.display());
                    all_right = false;
                } else if !managed_files.contains(&source.read_link().unwrap()) {
                    eprintln!(
                        "{} link does not point to a managed file: {}",
                        "Error:".red().bold(),
                        source.display()
                    );
                    all_right = false;
                }
            }
        }

        if !deploy_mode && self.warn_unmanaged(&self.root).1 {
            all_right = false;
        }

        all_right
    }

    // only warns about completely unmanaged items
    // if a directory contains any managed items, it is not warned about
    fn warn_unmanaged(&self, dir: &Path) -> (bool, bool) {
        let mut dir_contains_managed = false;
        let mut dir_contains_unmanaged = false; // only used in root call
        let mut unmanaged = Vec::new();
        let Ok(rd) = read_dir(dir) else { return (false, true) };
        for path in rd.flatten().map(|entry| entry.path()) {
            let is_managed = self.files.contains_key(&path) || self.ignored.contains(&path);
            dir_contains_managed |= is_managed; // base case
            if !is_managed {
                if path.is_dir() {
                    let is_empty = read_dir(&path).map_or(true, |mut rd| rd.next().is_none());
                    if is_empty {
                        dir_contains_unmanaged = true; // base case
                        unmanaged.push(path);
                        continue;
                    }

                    let (contains_managed, contains_unmanaged) = self.warn_unmanaged(&path);
                    dir_contains_managed |= contains_managed; // propagate up
                    dir_contains_unmanaged |= contains_unmanaged; // propagate up
                    if !contains_managed {
                        unmanaged.push(path);
                    }
                } else {
                    unmanaged.push(path);
                    dir_contains_unmanaged = true; // base case
                }
            }
        }

        if dir_contains_managed {
            for path in &unmanaged {
                if path.is_dir() {
                    eprintln!(
                        "{} directory is not managed: {}",
                        "Warning:".yellow().bold(),
                        path.display()
                    );
                } else {
                    eprintln!(
                        "{} file is not managed: {}",
                        "Warning:".yellow().bold(),
                        path.display()
                    );
                }
            }
        }

        (dir_contains_managed, dir_contains_unmanaged)
    }
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        create_dir_all(parent)?;
    }
    Ok(())
}

fn force_symlink(from: &Path, to: &Path) -> Result<()> {
    if to.is_symlink() || to.exists() {
        if to.is_dir() {
            remove_dir_all(to)?;
        } else {
            remove_file(to)?;
        }
    }
    symlink(from, to)?;
    Ok(())
}

fn home_dir() -> PathBuf {
    env::var_os("HOME").unwrap().into()
}

fn config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".config"))
}

fn dotfile_path() -> PathBuf {
    home_dir().join(".confast")
}

fn config_path() -> PathBuf {
    config_dir().join("confast/config.yaml")
}
