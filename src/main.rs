use std::collections::BTreeMap;
use std::env;
use std::fs::{File, create_dir_all, read_dir, remove_dir, remove_dir_all, remove_file, rename};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf, absolute};

use anyhow::{Result, ensure};
use clap::Parser;
use colored::Colorize;
use serde::{Deserialize, Serialize};

fn main() -> Result<()> {
    match Command::parse() {
        Command::Add { source, target } => add(source, target),
        Command::Mv { from, to } => mov(from, to),
        Command::Rm { target } => remove(target),
        Command::Check => check(),
        Command::Deploy { path, force } => deploy(path, force),
        Command::Init { path } => init(path),
    }
}

fn add(source: PathBuf, target: Option<PathBuf>) -> Result<()> {
    let mut cfg = Config::load(&config_file())?;
    let source = absolute(source)?;
    env::set_current_dir(&cfg.root)?;

    let source: PathBuf = source.components().collect(); // normalize path
    let target = target.unwrap_or(source.file_name().unwrap().into());
    ensure!(target.is_relative(), "target must be relative: {}", target.display());
    ensure!(!cfg.files.contains_key(&target), "config already contains file: {}", target.display());
    ensure!(source.exists(), "file does not exist: {}", source.display());
    ensure!(!target.exists(), "target already exists: {}", target.display());

    create_parent(&target)?;
    rename(&source, &target)?;
    force_symlink(&target, &source)?;
    cfg.files.insert(target, source);
    cfg.save(&config_file())?;
    Ok(())
}

fn mov(from: PathBuf, to: PathBuf) -> Result<()> {
    let mut cfg = Config::load(&config_file())?;
    env::set_current_dir(&cfg.root)?;

    ensure!(from.is_relative(), "paths must be relative: {}", from.display());
    ensure!(to.is_relative(), "paths must be relative: {}", to.display());
    ensure!(cfg.files.contains_key(&from), "config does not contain file: {}", from.display());
    ensure!(!cfg.files.contains_key(&to), "config already contains file: {}", to.display());
    ensure!(from.exists(), "file does not exist: {}", from.display());
    ensure!(!to.exists(), "file already exists: {}", to.display());

    rename(&from, &to)?;
    let source = cfg.files.remove(&from).unwrap();
    force_symlink(&to, &source)?;
    cfg.files.insert(to, source);
    cfg.save(&config_file())?;
    Ok(())
}

fn remove(target: PathBuf) -> Result<()> {
    let mut cfg = Config::load(&config_file())?;
    env::set_current_dir(&cfg.root)?;

    ensure!(target.is_relative(), "target must be relative: {}", target.display());
    ensure!(cfg.files.contains_key(&target), "config does not contain file: {}", target.display());
    ensure!(target.exists(), "file does not exist: {}", target.display());
    let original = cfg.files.remove(&target).unwrap();
    ensure!(
        original.read_link().is_ok_and(|p| p.strip_prefix(&cfg.root).is_ok_and(|p| p == target)),
        "original location is not a link pointing to target: {}",
        original.display()
    );

    remove_file(&original)?;
    rename(&target, &original)?;

    let parent = target.parent().unwrap();
    if parent != cfg.root
        && read_dir(parent).is_ok_and(|mut rd| rd.next().is_none())
        && !cfg.files.keys().any(|p| p.starts_with(parent))
    {
        remove_dir(parent)?;
        eprintln!("{} removing empty directory: {}", "Info:".green().bold(), parent.display());
    }

    cfg.save(&config_file())?;
    Ok(())
}

fn check() -> Result<()> {
    if Config::load(&config_file())?.check(false) {
        eprintln!("{} all good!", "Info:".green().bold());
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn deploy(root: Option<PathBuf>, force: bool) -> Result<()> {
    let root = root.unwrap_or(dotfiles_dir());
    ensure!(root.is_dir(), "path is not a directory");

    let cfg = Config::load(&root.join("confast/config.yaml"))?;
    ensure!(cfg.check(true), "please resolve these issues before continuing");

    for (target, source) in &cfg.files {
        if !source.exists() || force {
            create_parent(target)?;
            force_symlink(target, source)?;
        } else {
            eprintln!("{} skipping existing file: {}", "Info:".green(), source.display());
        }
    }
    Ok(())
}

fn init(root: Option<PathBuf>) -> Result<()> {
    let root = root.and_then(|p| absolute(p).ok()).unwrap_or(dotfiles_dir());
    let managed_cfg_dir = root.join("confast");
    let managed_cfg_file = managed_cfg_dir.join("config.yaml");
    ensure!(
        !managed_cfg_file.exists(),
        "config already exists in target directory: {}",
        managed_cfg_file.display()
    );

    let cfg_dir = config_dir().join("confast");
    create_parent(&cfg_dir)?;
    create_parent(&managed_cfg_file)?;
    force_symlink(&managed_cfg_dir, &cfg_dir)?;

    Config {
        root,
        ignored: vec![".git".into(), ".gitignore".into()],
        files: [(managed_cfg_dir, cfg_dir)].into(),
    }
    .save(&managed_cfg_file)?;
    Ok(())
}

#[derive(Parser)]
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
        /// Path to dotfiles directory, default is ~/.confast
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Overwrite existing files
        #[arg(short, long)]
        force: bool,
    },
    /// Initialize the program
    Init {
        /// Path to dotfiles directory, default is ~/.confast
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
}

// when loaded in program:
// `root` and `files.values` are made absolute
#[derive(Clone, Serialize, Deserialize)]
struct Config {
    root: PathBuf,
    ignored: Vec<PathBuf>,
    files: BTreeMap<PathBuf, PathBuf>,
}

impl Config {
    fn load(path: &Path) -> Result<Self> {
        let mut cfg: Self = yaml_serde::from_reader(File::open(path)?)?;
        let home = home_dir();
        cfg.root = home.join(cfg.root);
        cfg.files.values_mut().for_each(|p| *p = home.join(&p));
        Ok(cfg)
    }

    fn save(mut self, path: &Path) -> Result<()> {
        let home = home_dir();
        self.root = self.root.strip_prefix(&home).unwrap().to_owned();
        self.files.values_mut().for_each(|p| *p = p.strip_prefix(&home).unwrap().to_owned());
        yaml_serde::to_writer(File::create(path)?, &self)?;
        Ok(())
    }

    fn check(&self, deploy_mode: bool) -> bool {
        if env::set_current_dir(&self.root).is_err() {
            return false;
        }

        let mut all_right = true;
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
                } else if source
                    .canonicalize()
                    .unwrap()
                    .strip_prefix(&self.root)
                    .map_or(true, |p| !self.files.contains_key(p))
                {
                    eprintln!(
                        "{} link does not point to a managed file: {}",
                        "Error:".red().bold(),
                        source.display()
                    );
                    all_right = false;
                }
            }
        }

        if !deploy_mode && self.warn_unmanaged(Path::new(".")).1 {
            all_right = false;
        }

        all_right
    }

    // only warns about completely unmanaged items
    // if a directory contains any managed items, it is not warned about
    fn warn_unmanaged(&self, dir: &Path) -> (bool, bool) {
        let mut contains_managed = false;
        let mut contains_unmanaged = false; // only used in root call
        let mut unmanaged = Vec::new();
        let Ok(rd) = read_dir(dir) else { return (false, true) };
        for path in rd.flatten().map(|entry| entry.path()) {
            let path = path.strip_prefix(".").map(PathBuf::from).unwrap_or(path);
            let is_managed = self.files.contains_key(&path) || self.ignored.contains(&path);
            contains_managed |= is_managed; // base case
            if is_managed {
                continue;
            }

            if path.is_dir() {
                let is_empty = read_dir(&path).map_or(true, |mut rd| rd.next().is_none());
                if is_empty {
                    contains_unmanaged = true; // base case
                    unmanaged.push(path);
                    continue;
                }

                let (sub_contains_managed, sub_contains_unmanaged) = self.warn_unmanaged(&path);
                contains_managed |= sub_contains_managed; // propagate up
                contains_unmanaged |= sub_contains_unmanaged; // propagate up
                if !contains_managed {
                    unmanaged.push(path);
                }
            } else {
                contains_unmanaged = true; // base case
                unmanaged.push(path);
            }
        }

        if !contains_managed {
            for path in &unmanaged {
                let ty = if path.is_dir() { "directory" } else { "file" };
                eprintln!("{} {ty} is not managed: {}", "Warning:".yellow().bold(), path.display());
            }
        }

        (contains_managed, contains_unmanaged)
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
    let from = if from.is_absolute() { from } else { &absolute(from)? };
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
    env::home_dir().unwrap()
}

fn dotfiles_dir() -> PathBuf {
    home_dir().join(".confast")
}

fn config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".config"))
}

fn config_file() -> PathBuf {
    config_dir().join("confast/config.yaml")
}
