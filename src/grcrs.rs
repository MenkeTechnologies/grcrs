//! grc — the launcher half of grcrs.
//!
//! Faithful Rust port of grc's `grc` wrapper (Generic Colouriser 1.13). Parses
//! options, picks the grcat config file that matches the command line, runs the
//! command, and pipes its stdout/stderr through `grcat` for colourising.

use fancy_regex::Regex;
use std::env;
use std::fs;
use std::os::unix::io::FromRawFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn help() -> ! {
    println!(
        "Generic Colouriser 1.13
grc [options] command [args]
Options:
-e --stderr    redirect stderr. If this option is selected,
               do not automatically redirect stdout
-s --stdout    redirect stdout, even if -e is selected
-c name --config=name    use name as configuration file for grcat
--colour=word  word is one of: on, off, auto
--pty          run command in pseudoterminal (experimental)
"
    );
    std::process::exit(0);
}

/// Parsed command-line state.
struct Opts {
    stdoutf: bool,
    stderrf: bool,
    cfile: String,
    colour: bool,
    use_pty: bool,
    args: Vec<String>,
}

/// getopt-style parser: short `-sec`, long `--stdout/--stderr/--config/--colour/--pty`.
/// Option scanning stops at the first non-option argument (the command).
fn parse_opts(argv: &[String]) -> Opts {
    let mut o = Opts {
        stdoutf: false,
        stderrf: false,
        cfile: String::new(),
        colour: unsafe { libc::isatty(1) == 1 },
        use_pty: false,
        args: Vec::new(),
    };
    let mut i = 0;
    while i < argv.len() {
        let tok = &argv[i];
        if tok == "--" {
            i += 1;
            break;
        } else if let Some(long) = tok.strip_prefix("--") {
            let (name, inline_val) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            match name {
                "stdout" => o.stdoutf = true,
                "stderr" => o.stderrf = true,
                "pty" => o.use_pty = true,
                "config" => {
                    o.cfile = inline_val.unwrap_or_else(|| take_value(argv, &mut i));
                }
                "colour" => {
                    let v = inline_val.unwrap_or_else(|| take_value(argv, &mut i));
                    match v.as_str() {
                        "on" => o.colour = true,
                        "off" => o.colour = false,
                        "auto" => o.colour = unsafe { libc::isatty(1) == 1 },
                        _ => help(),
                    }
                }
                _ => help(),
            }
        } else if tok.starts_with('-') && tok.len() > 1 {
            let chars: Vec<char> = tok.chars().skip(1).collect();
            let mut j = 0;
            while j < chars.len() {
                match chars[j] {
                    's' => o.stdoutf = true,
                    'e' => o.stderrf = true,
                    'c' => {
                        // rest of the cluster, or the next argument, is the value
                        let rest: String = chars[j + 1..].iter().collect();
                        o.cfile = if !rest.is_empty() {
                            rest
                        } else {
                            take_value(argv, &mut i)
                        };
                        break;
                    }
                    _ => help(),
                }
                j += 1;
            }
        } else {
            break;
        }
        i += 1;
    }
    o.args = argv[i..].to_vec();
    o
}

/// Consume the next argument as an option value, advancing the index.
fn take_value(argv: &[String], i: &mut usize) -> String {
    if *i + 1 < argv.len() {
        *i += 1;
        argv[*i].clone()
    } else {
        help();
    }
}

/// Search the grc.conf files for a regexp matching the command line and return
/// the associated grcat config name, or empty if none matched.
fn find_config(args: &[String]) -> String {
    let cmdline = args.join(" ");
    let home = env::var("HOME").ok();
    let mut xdg = env::var("XDG_CONFIG_HOME").ok();
    if xdg.is_none() {
        if let Some(h) = &home {
            xdg = Some(format!("{}/.config", h));
        }
    }

    let mut conffiles = vec![
        "/etc/grc.conf".to_string(),
        "/usr/local/etc/grc.conf".to_string(),
        "/opt/homebrew/etc/grc.conf".to_string(),
    ];
    if let Some(x) = xdg {
        conffiles.push(format!("{}/grc/grc.conf", x));
    }
    if let Some(h) = &home {
        conffiles.push(format!("{}/.grc/grc.conf", h));
    }

    for conffile in conffiles {
        let path = PathBuf::from(&conffile);
        if !path.exists() || path.is_dir() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let mut idx = 0;
        while idx < lines.len() {
            let l = lines[idx];
            idx += 1;
            if l.starts_with('#') || l.is_empty() {
                continue;
            }
            let regexp = translate_regex(l.trim());
            if let Ok(re) = Regex::new(&regexp) {
                if re.is_match(&cmdline).unwrap_or(false) {
                    // The next line names the grcat config file.
                    if idx < lines.len() {
                        return lines[idx].trim().to_string();
                    }
                    return String::new();
                }
            }
        }
    }
    String::new()
}

/// Translate Python-`re` regexp source to fancy-regex dialect (literal `\<`/`\>`).
fn translate_regex(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'<' | b'>' => {
                    out.push(bytes[i + 1] as char);
                    i += 2;
                }
                _ => {
                    out.push('\\');
                    let ch = src[i + 1..].chars().next().unwrap();
                    out.push(ch);
                    i += 1 + ch.len_utf8();
                }
            }
        } else {
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Locate the grcat binary: alongside this executable if present, else on PATH.
fn grcat_path() -> PathBuf {
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("grcat");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from("grcat")
}

fn main() {
    let argv: Vec<String> = env::args().skip(1).collect();
    let o = parse_opts(&argv);
    if o.args.is_empty() {
        help();
    }

    // Resolve stdout/stderr redirection flags exactly as grc does.
    let mut stdoutff = true;
    let mut stderrff = false;
    if o.stderrf {
        stdoutff = false;
        stderrff = true;
    }
    if o.stdoutf {
        stdoutff = true;
    }

    let cfile = if o.cfile.is_empty() {
        find_config(&o.args)
    } else {
        o.cfile.clone()
    };

    if !cfile.is_empty() && o.colour {
        std::process::exit(run_coloured(&o.args, stdoutff, stderrff, &cfile, o.use_pty));
    } else {
        std::process::exit(run_plain(&o.args));
    }
}

/// Run the command with inherited stdio and propagate its exit status.
fn run_plain(args: &[String]) -> i32 {
    match Command::new(&args[0]).args(&args[1..]).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("grc: {}: {}", args[0], strerror(&e));
            1
        }
    }
}

/// Run the command, piping its stdout/stderr through grcat.
fn run_coloured(
    args: &[String],
    stdoutff: bool,
    stderrff: bool,
    cfile: &str,
    use_pty: bool,
) -> i32 {
    // Ignore SIGINT in the wrapper so Ctrl-C reaches the child instead of
    // killing grc before it can reap and report the child's status.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }

    let grcat = grcat_path();

    // pty mode: connect the command's stdout to a pseudo-terminal so it emits
    // tty-style output, and feed the master end to grcat. (experimental)
    let mut pty_master: Option<i32> = None;

    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..]);

    if stdoutff {
        if use_pty {
            match open_pty() {
                Some((master, slave)) => {
                    pty_master = Some(master);
                    // SAFETY: slave is a fresh, owned fd from openpty.
                    cmd.stdout(unsafe { Stdio::from_raw_fd(slave) });
                }
                None => {
                    cmd.stdout(Stdio::piped());
                }
            };
        } else {
            cmd.stdout(Stdio::piped());
        }
    }
    if stderrff {
        cmd.stderr(Stdio::piped());
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("grc: {}: {}", args[0], strerror(&e));
            return 1;
        }
    };

    let mut grcat_children = Vec::new();

    if stdoutff {
        let stdin_fd: Stdio = if let Some(master) = pty_master {
            // SAFETY: master is an owned fd returned by openpty.
            unsafe { Stdio::from_raw_fd(master) }
        } else {
            Stdio::from(child.stdout.take().expect("piped stdout"))
        };
        if let Ok(c) = Command::new(&grcat).arg(cfile).stdin(stdin_fd).spawn() {
            grcat_children.push(c);
        }
    }

    if stderrff {
        // grcat reads the command's stderr and writes to grc's stderr (fd 2).
        let out_fd = unsafe { libc::dup(2) };
        let stdout_target = unsafe { Stdio::from_raw_fd(out_fd) };
        if let Ok(c) = Command::new(&grcat)
            .arg(cfile)
            .stdin(Stdio::from(child.stderr.take().expect("piped stderr")))
            .stdout(stdout_target)
            .spawn()
        {
            grcat_children.push(c);
        }
    }

    let status = child.wait().ok();
    for mut g in grcat_children {
        let _ = g.wait();
    }

    status.and_then(|s| s.code()).unwrap_or(0)
}

/// Allocate a pseudo-terminal, returning (master_fd, slave_fd).
fn open_pty() -> Option<(i32, i32)> {
    let mut master: i32 = 0;
    let mut slave: i32 = 0;
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc == 0 {
        Some((master, slave))
    } else {
        None
    }
}

/// The bare OS error string, matching grc's `%s: %s` error format.
fn strerror(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(code) => {
            let s = unsafe { libc::strerror(code) };
            if s.is_null() {
                e.to_string()
            } else {
                unsafe { std::ffi::CStr::from_ptr(s) }
                    .to_string_lossy()
                    .into_owned()
            }
        }
        None => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_opts_stops_at_command() {
        let o = parse_opts(&s(&["-s", "ls", "-l"]));
        assert!(o.stdoutf);
        assert_eq!(o.args, s(&["ls", "-l"]));
    }

    #[test]
    fn parse_opts_long_forms() {
        let o = parse_opts(&s(&["--stderr", "--config=conf.foo", "cmd"]));
        assert!(o.stderrf);
        assert_eq!(o.cfile, "conf.foo");
        assert_eq!(o.args, s(&["cmd"]));
    }

    #[test]
    fn parse_opts_config_separate_and_attached() {
        assert_eq!(parse_opts(&s(&["-c", "conf.x", "cmd"])).cfile, "conf.x");
        assert_eq!(parse_opts(&s(&["-cconf.x", "cmd"])).cfile, "conf.x");
        assert_eq!(
            parse_opts(&s(&["--config", "conf.x", "cmd"])).cfile,
            "conf.x"
        );
    }

    #[test]
    fn parse_opts_clustered_shorts_and_pty() {
        let o = parse_opts(&s(&["-se", "--pty", "top"]));
        assert!(o.stdoutf && o.stderrf && o.use_pty);
        assert_eq!(o.args, s(&["top"]));
    }

    #[test]
    fn parse_opts_double_dash_terminates_options() {
        // Everything after `--` is the command, even if it looks like a flag.
        let o = parse_opts(&s(&["--", "-s", "notaflag"]));
        assert!(!o.stdoutf);
        assert_eq!(o.args, s(&["-s", "notaflag"]));
    }

    #[test]
    fn parse_opts_colour_flag() {
        assert!(parse_opts(&s(&["--colour=on", "cmd"])).colour);
        assert!(!parse_opts(&s(&["--colour=off", "cmd"])).colour);
    }

    #[test]
    fn translate_regex_matches_grcat_behaviour() {
        assert_eq!(translate_regex(r"^\>(.*)"), "^>(.*)");
        assert_eq!(translate_regex(r"\d+"), r"\d+");
    }
}
