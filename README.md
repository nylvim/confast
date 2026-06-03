# confast

A minimalistic dotfile manager.

## Usage

The term "file" can refer to a regular file or a directory.

### Initialize the program

Default dotfiles directory is `~/.confast`.

```
confast init [PATH]
```

### Deploy dotfiles

Link dotfiles to recorded locations.

Use `-f`/`--force` to overwrite existing files.

```
confast deploy [PATH] [-f]
```

### Add a new managed file

Move the file to the dotfiles directory and create a soft link in the original location.

Target path is relative to dotfiles directory.

```
confast add <SOURCE> [TARGET]
```

### Move a managed file

Move the file and update the config so you don't have to do it manually.

Paths are relative to dotfiles directory.

```
confast mv <FROM> <TO>
```

### Unmanage a file

Move the file to its originial location.

Path is relative to dotfiles directory.

```
confast rm <TARGET>
```

### Check the config

```
confast check
```

## Why use Confast?

If you're me.

Seriously, I made this program simply because every existing alternative is either too simple that is hard to use (like GNU Stow) or contains too much features that I won't ever need (like chezmoi).

"Confast" is at first just the name (that I came up in like 5 seconds) of the folder storing my dotfiles. I used to manually move and link them for months before I decided to build an automated tool.
