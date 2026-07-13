```
 ██████╗ ██████╗  ██████╗██████╗ ███████╗
██╔════╝ ██╔══██╗██╔════╝██╔══██╗██╔════╝
██║  ███╗██████╔╝██║     ██████╔╝███████╗
██║   ██║██╔══██╗██║     ██╔══██╗╚════██║
╚██████╔╝██║  ██║╚██████╗██║  ██║███████║
 ╚═════╝ ╚═╝  ╚═╝ ╚═════╝╚═╝  ╚═╝╚══════╝
```

[![CI](https://github.com/MenkeTechnologies/grcrs/actions/workflows/ci.yml/badge.svg)](https://github.com/MenkeTechnologies/grcrs/actions/workflows/ci.yml)
[![Release](https://github.com/MenkeTechnologies/grcrs/actions/workflows/release.yml/badge.svg)](https://github.com/MenkeTechnologies/grcrs/actions/workflows/release.yml)
 [![Docs](https://img.shields.io/badge/docs-online-05d9e8.svg)](https://menketechnologies.github.io/grcrs/)
[![License: GPL-2.0-or-later](https://img.shields.io/badge/License-GPL--2.0--or--later-blue.svg)](https://www.gnu.org/licenses/gpl-2.0.html)

### `[GENERIC COLOURISER // ANSI STREAM PAINTER // REGEXP-DRIVEN // RUST CORE]`

> *"Every stream has a colour. Find the pattern. Paint the output."*

`grcrs` is a faithful Rust port of [grc](https://github.com/garabik/grc) (Generic Colouriser 1.13). It ships two binaries that mirror the original pair:

- **`grc`** — the launcher. Parses options, matches the command line against `grc.conf`, runs the command, and pipes its stdout/stderr through `grcat`.
- **`grcat`** — the colouriser. Reads a config file of regexp/colour blocks, reads stdin line by line, applies the matching ANSI colours, and writes to stdout.

 ┌──────────────────────────────────────────────────────────────┐
 │ STATUS: ONLINE &nbsp;&nbsp; MODE: STREAM-PAINT &nbsp;&nbsp; ENGINE: fancy-regex │
 └──────────────────────────────────────────────────────────────┘

### [`Read the Docs`](https://menketechnologies.github.io/grcrs/) &middot; [`Engineering Report`](https://menketechnologies.github.io/grcrs/report.html) · [`strykelang`](https://github.com/MenkeTechnologies/strykelang) · [`zshrs`](https://github.com/MenkeTechnologies/zshrs)

---

## Table of Contents

- [\[0x00\] What It Does](#0x00-what-it-does)
- [\[0x01\] System Requirements](#0x01-system-requirements)
- [\[0x02\] Installation](#0x02-installation)
- [\[0x03\] Usage](#0x03-usage)
- [\[0x04\] Config Search Paths](#0x04-config-search-paths)
- [\[0x05\] How Matching Works](#0x05-how-matching-works)
- [\[0x06\] Colourfile Format](#0x06-colourfile-format)
- [\[0x07\] Development & CI](#0x07-development--ci)
- [\[0xFF\] License](#0xff-license)

---

## [0x00] WHAT IT DOES

`grc <command>` runs a command and colourises its output. It searches `grc.conf` for the first regexp that matches the command line, loads the named colourfile, runs the command, and streams the output through `grcat` where regexp-driven ANSI colouring is applied per line. If no regexp matches, the command runs uncoloured.

```sh
grc ping example.com      # colourised ping
grc netstat -tulpn        # colourised netstat
grc df -h                 # colourised disk usage
grc ifconfig              # colourised interface dump
grc make                  # colourised build log
```

The port is faithful to grc 1.13 semantics: the same option surface, the same config-file search order, the same colourfile grammar (`regexp` / `colours` / `count` / `command` / `skip` / `replace` / `concat`), the same loop directives (`more`, `once`, `stop`, `block`, `unblock`, `previous`), and the same `block`/`prev`/`unchanged` streaming-colour behaviour carried across lines.

---

## [0x01] SYSTEM REQUIREMENTS

- Rust toolchain // `rustc` + `cargo`
- A `grc.conf` + colourfiles on one of the search paths (bundled in the `vendor/grc` submodule, installed by the Homebrew formula)

## [0x02] INSTALLATION

#### HOMEBREW TAP (RECOMMENDED)

```sh
brew install menketechnologies/menketech/grcrs
```

The formula installs the two binaries plus `grc.conf` (to `$(brew --prefix)/etc`) and the colourfiles (to `$(brew --prefix)/share/grc`), which `grc`/`grcat` load at runtime.

#### COMPILING FROM SOURCE

```sh
git clone --recurse-submodules https://github.com/MenkeTechnologies/grcrs
cd grcrs
cargo build --release
```

The runtime config lives in the `vendor/grc` submodule (`grc.conf` and `colourfiles/conf.*`). Install `grc.conf` to one of `/etc`, `/usr/local/etc`, `/opt/homebrew/etc`, `$XDG_CONFIG_HOME/grc`, or `~/.grc`, and the colourfiles to a matching `share/grc` search directory.

---

## [0x03] USAGE

```sh
grc [options] command [args]
```

#### OPTIONS

| Option | Behavior |
|--------|----------|
| `-e`, `--stderr` | Redirect stderr. When set, stdout is not automatically redirected. |
| `-s`, `--stdout` | Redirect stdout, even if `-e` is selected. |
| `-c name`, `--config=name` | Use `name` as the grcat colourfile instead of matching `grc.conf`. |
| `--colour=WORD` | `on`, `off`, or `auto` (default: on when stdout is a tty). |
| `--pty` | Run the command in a pseudo-terminal so it emits tty-style output (experimental). |

#### EXAMPLES

```sh
# colourise a command's stdout (auto-matched against grc.conf)
grc ping example.com

# colourise stderr instead of stdout
grc -e make

# colourise both
grc -s -e ./build.sh

# force a specific colourfile, skipping grc.conf matching
grc -c conf.log tail -f /var/log/system.log
grc --config=conf.df df -h

# control colour explicitly
grc --colour=on  ls -l     # force colour even when piped
grc --colour=off netstat   # disable colour
grc --colour=auto ping x    # colour only when stdout is a tty

# run under a pty so the command thinks it's interactive (experimental)
grc --pty top
```

`grcat` is not meant to be called directly — `grc` pipes a command's output into it — but it takes a single colourfile argument and reads stdin:

```sh
some-command | grcat conf.log
```

---

## [0x04] CONFIG SEARCH PATHS

Both binaries resolve their config the same way grc 1.13 does, in this order:

| File | Searched in (in order) |
|------|------------------------|
| `grc.conf` | `/etc`, `/usr/local/etc`, `/opt/homebrew/etc`, `$XDG_CONFIG_HOME/grc`, `~/.grc` |
| colourfiles | `$XDG_DATA_HOME/grc`, `$XDG_CONFIG_HOME/grc`, `~/.grc`, `/usr/local/share/grc`, `/usr/share/grc`, `/opt/homebrew/share/grc` |

`$XDG_CONFIG_HOME` defaults to `~/.config` and `$XDG_DATA_HOME` to `~/.local/share` when unset. The first existing file wins; the colourfile path also includes the empty prefix, so an absolute or relative colourfile name passed to `-c` is honoured as-is.

---

## [0x05] HOW MATCHING WORKS

```
 ┌──────────────────────────────────────────────────────────┐
 │  grc ping example.com                                    │
 │        │                                                 │
 │        ▼  join argv → "ping example.com"                 │
 │  grc.conf ── first matching regexp ──▶ conf.ping          │
 │        │                                                 │
 │        ▼  spawn command, pipe stdout/stderr              │
 │  grcat conf.ping ── per-line regexp colouring ──▶ tty     │
 └──────────────────────────────────────────────────────────┘
```

- `grc` joins the command + args into one string and scans `grc.conf` top to bottom. Each non-comment, non-blank line is a Python-`re` regexp; the **next** line names the grcat colourfile to use on a match.
- Python `\<` / `\>` (literal `<` / `>` in Python's `re`) are translated to literals so configs authored for grc behave identically under `fancy-regex`.
- On a match with colour enabled, `grc` runs the command and pipes its output through `grcat <colourfile>`; otherwise the command runs uncoloured with inherited stdio.
- `SIGINT` is ignored in the wrapper so Ctrl-C reaches the child, and `grc` still reaps and reports the child's exit status.

---

## [0x06] COLOURFILE FORMAT

A colourfile is a sequence of blocks. A block is a run of `keyword=value` lines; any line not starting with `#`, a blank line, or a line whose first character is not an ASCII letter ends the block.

| Keyword | Meaning |
|---------|---------|
| `regexp` | The pattern to match on each input line. |
| `colours` | Comma-separated colour list, one entry per capture group. Space-joined tokens compose (`bold red`). |
| `count` | Loop directive: `more` (default), `once`, `stop`, `block`, `unblock`, `previous`. |
| `command` | Shell command to run via `sh -c` on a match. |
| `skip` | `yes`/`1`/`true` suppresses the line from output. |
| `replace` | Replacement string; Python `\N` backrefs become `${N}`. |
| `concat` | Append the matched line to a file. |

Colour tokens resolve through a static ANSI table — `red`, `bold`, `on_blue`, `bright_cyan`, `italic`, `underline`, `previous`, `unchanged`, `none`, `default` — or a `"..."` quoted literal that is unescaped like a Python string (octal `\033`, hex `\xNN`, and the common single-char escapes). Bright colours are emitted with a standard-code prefix for graceful fallback on terminals without aixterm codes.

The bundled `vendor/grc` submodule ships **83 colourfiles** (`conf.ant` … `conf.yaml`) and a `grc.conf` mapping commands to them.

---

## [0x07] DEVELOPMENT & CI

Pushes to `main` and pull requests run [`.github/workflows/ci.yml`](.github/workflows/ci.yml): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc -D warnings`, and a build + test on both `ubuntu-latest` and `macos-latest`, plus a binary smoke test. You can also run it manually from the repository **Actions** tab (**workflow dispatch**).

The two binaries build from `src/grcrs.rs` (`grc`, the launcher) and `src/grcatrs.rs` (`grcat`, the colouriser). The release profile uses LTO + `codegen-units = 1`.

---

## [0xFF] LICENSE

 ┌──────────────────────────────────────────────────────────┐
 │ GPL-2.0-or-later // SAME LICENSE AS UPSTREAM grc         │
 └──────────────────────────────────────────────────────────┘

GPL-2.0-or-later — matching upstream [grc](https://github.com/garabik/grc).

---

```
░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
░░ >>> PIPE IT IN. MATCH THE PATTERN. PAINT THE STREAM. <<< ░░
░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
```

##### created by [MenkeTechnologies](https://github.com/MenkeTechnologies)
