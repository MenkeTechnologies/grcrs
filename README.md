# grcrs

[![CI](https://github.com/MenkeTechnologies/grcrs/actions/workflows/ci.yml/badge.svg)](https://github.com/MenkeTechnologies/grcrs/actions/workflows/ci.yml)
[![Release](https://github.com/MenkeTechnologies/grcrs/actions/workflows/release.yml/badge.svg)](https://github.com/MenkeTechnologies/grcrs/actions/workflows/release.yml)
[![License: GPL-2.0-or-later](https://img.shields.io/badge/License-GPL--2.0--or--later-blue.svg)](https://www.gnu.org/licenses/gpl-2.0.html)

Generic Colouriser — a faithful Rust port of [grc](https://github.com/garabik/grc) (Generic Colouriser 1.13).

Ships two binaries:

- `grc` — the launcher: parses options, matches the command line against `grc.conf`, runs the command, and pipes its output through `grcat`.
- `grcat` — the colouriser: reads a config file and applies regexp-driven ANSI colouring to stdin.

## Install

### Homebrew

```sh
brew install menketechnologies/menketech/grcrs
```

The formula installs the two binaries plus `grc.conf` (to `$(brew --prefix)/etc`) and the colourfiles (to `$(brew --prefix)/share/grc`), which `grc`/`grcat` load at runtime.

### From source

```sh
git clone --recurse-submodules https://github.com/MenkeTechnologies/grcrs
cd grcrs
cargo build --release
```

The runtime config lives in the `vendor/grc` submodule (`grc.conf` and `colourfiles/conf.*`). Install `grc.conf` to one of `/etc`, `/usr/local/etc`, `/opt/homebrew/etc`, `$XDG_CONFIG_HOME/grc`, or `~/.grc`, and the colourfiles to a matching `share/grc` search directory.

## Usage

```sh
grc <command> [args]        # colourise a command's output
grc -e <command>            # colourise stderr instead of stdout
grc -c <name> <command>     # force a specific grcat config
grc --colour=on <command>   # on | off | auto
```

`grc` searches for the first regexp in `grc.conf` that matches the command line and uses the named colourfile. If none matches, the command runs uncoloured.

## Config search paths

| File | Searched in (in order) |
|------|------------------------|
| `grc.conf` | `/etc`, `/usr/local/etc`, `/opt/homebrew/etc`, `$XDG_CONFIG_HOME/grc`, `~/.grc` |
| colourfiles | `$XDG_DATA_HOME/grc`, `$XDG_CONFIG_HOME/grc`, `~/.grc`, `/usr/local/share/grc`, `/usr/share/grc`, `/opt/homebrew/share/grc` |

## License

GPL-2.0-or-later.
