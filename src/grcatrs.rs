//! grcatrs — the colouriser half of grcrs.
//!
//! Faithful Rust port of grc's `grcat` (Generic Colouriser 1.13). Reads a
//! configuration file describing regexp/colour blocks, then reads stdin line by
//! line, applies the matching colours, and writes the result to stdout.
//!
//! Not meant to be called directly; grcrs pipes a command's output into it.

use fancy_regex::{Captures, Regex};
use std::collections::HashMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Command;

/// One parsed configuration block: a compiled regexp plus its directives.
struct Pattern {
    regexp: Regex,
    /// One resolved colour string per comma-separated group, or `None`.
    colours: Option<Vec<String>>,
    /// Loop directive: "more" (default), "once", "stop", "block", "unblock", "previous".
    count: String,
    command: Option<String>,
    skip: Option<String>,
    /// Replacement string, converted from `\N` backrefs to `${N}`.
    replace: Option<String>,
    concat: Option<String>,
}

/// Static ANSI colour/attribute table, mirroring grcat's `colours` dict.
fn colour_table() -> HashMap<&'static str, &'static str> {
    [
        ("none", ""),
        ("default", "\x1b[0m"),
        ("bold", "\x1b[1m"),
        ("underline", "\x1b[4m"),
        ("blink", "\x1b[5m"),
        ("reverse", "\x1b[7m"),
        ("concealed", "\x1b[8m"),
        ("black", "\x1b[30m"),
        ("red", "\x1b[31m"),
        ("green", "\x1b[32m"),
        ("yellow", "\x1b[33m"),
        ("blue", "\x1b[34m"),
        ("magenta", "\x1b[35m"),
        ("cyan", "\x1b[36m"),
        ("white", "\x1b[37m"),
        ("on_black", "\x1b[40m"),
        ("on_red", "\x1b[41m"),
        ("on_green", "\x1b[42m"),
        ("on_yellow", "\x1b[43m"),
        ("on_blue", "\x1b[44m"),
        ("on_magenta", "\x1b[45m"),
        ("on_cyan", "\x1b[46m"),
        ("on_white", "\x1b[47m"),
        ("beep", "\x07"),
        ("previous", "prev"),
        ("unchanged", "unchanged"),
        // non-standard attributes, supported by some terminals
        ("dark", "\x1b[2m"),
        ("italic", "\x1b[3m"),
        ("rapidblink", "\x1b[6m"),
        ("strikethrough", "\x1b[9m"),
        // aixterm bright color codes (prefixed with standard codes for graceful failure)
        ("bright_black", "\x1b[30;90m"),
        ("bright_red", "\x1b[31;91m"),
        ("bright_green", "\x1b[32;92m"),
        ("bright_yellow", "\x1b[33;93m"),
        ("bright_blue", "\x1b[34;94m"),
        ("bright_magenta", "\x1b[35;95m"),
        ("bright_cyan", "\x1b[36;96m"),
        ("bright_white", "\x1b[37;97m"),
        ("on_bright_black", "\x1b[40;100m"),
        ("on_bright_red", "\x1b[41;101m"),
        ("on_bright_green", "\x1b[42;102m"),
        ("on_bright_yellow", "\x1b[43;103m"),
        ("on_bright_blue", "\x1b[44;104m"),
        ("on_bright_magenta", "\x1b[45;105m"),
        ("on_bright_cyan", "\x1b[46;106m"),
        ("on_bright_white", "\x1b[47;107m"),
    ]
    .into_iter()
    .collect()
}

/// Resolve a single colour token to its escape string.
///
/// A named colour maps through the table; a `"..."` quoted literal is
/// unescaped (grcat evaluates it as a Python string literal).
fn get_colour(x: &str, table: &HashMap<&'static str, &'static str>) -> Result<String, String> {
    if let Some(v) = table.get(x) {
        Ok((*v).to_string())
    } else if x.len() >= 2 && x.starts_with('"') && x.ends_with('"') {
        Ok(unescape(&x[1..x.len() - 1]))
    } else {
        Err(format!("Bad colour specified: {}", x))
    }
}

/// Decode the escape sequences a Python string literal would (octal `\033`,
/// hex `\xNN`, and the common single-char escapes).
fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let c = bytes[i + 1];
            match c {
                b'n' => {
                    out.push('\n');
                    i += 2;
                }
                b'r' => {
                    out.push('\r');
                    i += 2;
                }
                b't' => {
                    out.push('\t');
                    i += 2;
                }
                b'a' => {
                    out.push('\x07');
                    i += 2;
                }
                b'b' => {
                    out.push('\x08');
                    i += 2;
                }
                b'f' => {
                    out.push('\x0c');
                    i += 2;
                }
                b'v' => {
                    out.push('\x0b');
                    i += 2;
                }
                b'\\' => {
                    out.push('\\');
                    i += 2;
                }
                b'"' => {
                    out.push('"');
                    i += 2;
                }
                b'\'' => {
                    out.push('\'');
                    i += 2;
                }
                b'0'..=b'7' => {
                    let mut j = i + 1;
                    let mut val: u32 = 0;
                    let mut n = 0;
                    while j < bytes.len() && n < 3 && (b'0'..=b'7').contains(&bytes[j]) {
                        val = val * 8 + (bytes[j] - b'0') as u32;
                        j += 1;
                        n += 1;
                    }
                    out.push(char::from_u32(val).unwrap_or('\u{FFFD}'));
                    i = j;
                }
                b'x' => {
                    let mut j = i + 2;
                    let mut val: u32 = 0;
                    let mut n = 0;
                    while j < bytes.len() && n < 2 && bytes[j].is_ascii_hexdigit() {
                        val = val * 16 + (bytes[j] as char).to_digit(16).unwrap();
                        j += 1;
                        n += 1;
                    }
                    out.push(char::from_u32(val).unwrap_or('\u{FFFD}'));
                    i = j;
                }
                _ => {
                    out.push('\\');
                    i += 1;
                }
            }
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Build the ordered search path grcat uses to locate a config file by name.
fn conf_search_path() -> Vec<String> {
    let home = env::var("HOME").ok();
    let mut xdg_config = env::var("XDG_CONFIG_HOME").ok();
    let mut xdg_data = env::var("XDG_DATA_HOME").ok();
    if let Some(h) = &home {
        if xdg_config.is_none() {
            xdg_config = Some(format!("{}/.config", h));
        }
        if xdg_data.is_none() {
            xdg_data = Some(format!("{}/.local/share", h));
        }
    }
    let mut path = vec![String::new()];
    if let Some(d) = xdg_data {
        path.push(format!("{}/grc/", d));
    }
    if let Some(c) = xdg_config {
        path.push(format!("{}/grc/", c));
    }
    if let Some(h) = &home {
        path.push(format!("{}/.grc/", h));
    }
    path.push("/usr/local/share/grc/".to_string());
    path.push("/usr/share/grc/".to_string());
    path.push("/opt/homebrew/share/grc/".to_string());
    path
}

/// Parse a config file into an ordered list of patterns. Blocks are separated
/// by any line not starting with `#`, a letter, or being blank.
fn parse_config(
    text: &str,
    table: &HashMap<&'static str, &'static str>,
) -> Result<Vec<Pattern>, String> {
    const KEYWORDS: [&str; 7] = [
        "regexp", "colours", "count", "command", "skip", "replace", "concat",
    ];
    let mut patterns = Vec::new();
    let mut lines = text.lines();
    let mut is_last = false;
    while !is_last {
        let mut block: HashMap<String, String> = HashMap::new();
        block.insert("count".to_string(), "more".to_string());
        loop {
            match lines.next() {
                None => {
                    is_last = true;
                    break;
                }
                Some(l) => {
                    if l.starts_with('#') || l.is_empty() {
                        continue;
                    }
                    // A line not beginning with an ASCII letter ends the block.
                    if !l.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                        break;
                    }
                    let line = l.trim_end_matches(['\r', '\n']);
                    let (kw, val) = match line.split_once('=') {
                        Some(kv) => kv,
                        None => {
                            return Err(format!(
                                "Error in configuration, I expect keyword=value line\nBut I got instead:\n{:?}\n",
                                line
                            ));
                        }
                    };
                    let mut keyword = kw.to_ascii_lowercase();
                    if keyword == "colors" || keyword == "colour" || keyword == "color" {
                        keyword = "colours".to_string();
                    }
                    if !KEYWORDS.contains(&keyword.as_str()) {
                        return Err("Invalid keyword".to_string());
                    }
                    block.insert(keyword, val.to_string());
                }
            }
        }

        // Resolve the colour lists (one entry per comma-separated group).
        let colours = match block.get("colours") {
            Some(cv) => {
                let mut groups = Vec::new();
                for colgroup in cv.split(',') {
                    let mut s = String::new();
                    for tok in colgroup.split_whitespace() {
                        s.push_str(&get_colour(tok, table)?);
                    }
                    groups.push(s);
                }
                Some(groups)
            }
            None => None,
        };

        if let Some(re_src) = block.get("regexp") {
            let regexp = Regex::new(&translate_regex(re_src))
                .map_err(|e| format!("bad regexp {:?}: {}", re_src, e))?;
            patterns.push(Pattern {
                regexp,
                colours,
                count: block
                    .get("count")
                    .cloned()
                    .unwrap_or_else(|| "more".to_string()),
                command: block.get("command").cloned(),
                skip: block.get("skip").cloned(),
                replace: block.get("replace").map(|r| convert_backrefs(r)),
                concat: block.get("concat").cloned(),
            });
        }
    }
    Ok(patterns)
}

/// Translate Python-`re` regexp source to fancy-regex dialect.
///
/// Python treats `\<` and `\>` as the literal characters `<`/`>` (it has no
/// word-boundary form for them), whereas fancy-regex reads them as GNU-style
/// word boundaries. Rewrite them to literals so grc configs behave as authored.
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

/// Convert Python-style `\1` backrefs in a replacement to the `${1}` form the
/// regex engine expects. Literal `$` is escaped to `$$`.
fn convert_backrefs(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() => {
                out.push_str("${");
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    out.push(bytes[j] as char);
                    j += 1;
                }
                out.push('}');
                i = j;
            }
            b'$' => {
                out.push_str("$$");
                i += 1;
            }
            _ => {
                let ch = s[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// Convert a byte offset within `line` to a character index, clamping to the
/// string length. Match offsets always land on char boundaries.
fn byte_to_char(line: &str, b: usize) -> usize {
    let b = b.min(line.len());
    line[..b].chars().count()
}

/// Capture group byte spans, `None` for non-participating groups.
fn group_spans(caps: &Captures) -> Vec<Option<(usize, usize)>> {
    (0..caps.len())
        .map(|g| caps.get(g).map(|m| (m.start(), m.end())))
        .collect()
}

/// Append every capture group to `clist` as (char_start, char_end, colour).
/// Groups beyond the colour list reuse colour 0; non-participating groups are
/// skipped (equivalent to grcat's harmless (-1,-1) entries).
fn add2list(
    clist: &mut Vec<(usize, usize, String)>,
    groups: &[Option<(usize, usize)>],
    cols: &[String],
    line: &str,
) {
    for (group, span) in groups.iter().enumerate() {
        if let Some((start, end)) = *span {
            let colour = if group < cols.len() {
                &cols[group]
            } else {
                &cols[0]
            };
            clist.push((
                byte_to_char(line, start),
                byte_to_char(line, end),
                colour.clone(),
            ));
        }
    }
}

fn main() {
    // Ignore Ctrl-C so grcrs can forward SIGINT to the wrapped command instead.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }

    let argv: Vec<String> = env::args().collect();
    if argv.len() != 2 {
        eprintln!(
            "You are not supposed to call grcatrs directly, but the usage is: grcatrs conffile"
        );
        std::process::exit(1);
    }
    let conffile_arg = &argv[1];

    let mut conffile: Option<PathBuf> = None;
    for prefix in conf_search_path() {
        let candidate = PathBuf::from(format!("{}{}", prefix, conffile_arg));
        if candidate.exists() && !candidate.is_dir() {
            conffile = Some(candidate);
            break;
        }
    }
    let conffile = match conffile {
        Some(c) => c,
        None => {
            eprintln!("config file [{}] not found", conffile_arg);
            std::process::exit(1);
        }
    };

    let text = match fs::read_to_string(&conffile) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("grcatrs: {}: {}", conffile.display(), e);
            std::process::exit(1);
        }
    };

    let table = colour_table();
    let default = table["default"].to_string();
    let patterns = match parse_config(&text, &table) {
        Ok(p) => p,
        Err(e) => {
            eprint!("{}", e);
            std::process::exit(1);
        }
    };

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = io::BufWriter::new(stdout.lock());

    // Streaming state carried across lines.
    let mut prevcolour = default.clone();
    let mut prevcount = "more".to_string();
    let mut blockflag = false;
    let mut blockcolour = default.clone();

    let mut raw: Vec<u8> = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        // Decode leniently; command output is not guaranteed valid UTF-8.
        let mut line = String::from_utf8_lossy(&raw).into_owned();
        // Strip a single trailing newline character (mirrors grcat).
        if line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }

        // clist: (char_start, char_end, colour_string)
        let mut clist: Vec<(usize, usize, String)> = Vec::new();
        let mut skip = false;

        for pattern in &patterns {
            let mut pos = 0usize;
            let mut currcount = pattern.count.clone();
            let mut was_replace = false;
            let mut m_present;
            loop {
                if pos > line.len() {
                    m_present = false;
                    break;
                }
                let caps = match pattern.regexp.captures_from_pos(&line, pos) {
                    Ok(Some(c)) => c,
                    Ok(None) | Err(_) => {
                        m_present = false;
                        break;
                    }
                };
                m_present = true;
                let groups = group_spans(&caps);
                let mend = groups[0].unwrap().1;

                if let Some(rep) = pattern.replace.as_ref() {
                    if was_replace {
                        break;
                    }
                    line = pattern.regexp.replace_all(&line, rep.as_str()).into_owned();
                    was_replace = true;
                }

                if let Some(cols) = &pattern.colours {
                    if currcount == "block" {
                        blockflag = true;
                        blockcolour = cols[0].clone();
                        break;
                    } else if currcount == "unblock" {
                        blockflag = false;
                        blockcolour = default.clone();
                        currcount = "stop".to_string();
                    }
                    add2list(&mut clist, &groups, cols, &line);
                    if currcount == "previous" {
                        currcount = prevcount.clone();
                    }
                    if currcount == "stop" {
                        break;
                    }
                    if currcount == "more" {
                        prevcount = "more".to_string();
                        if mend == pos {
                            // zero-width match: advance to next char boundary to escape
                            let mut p = pos + 1;
                            while p < line.len() && !line.is_char_boundary(p) {
                                p += 1;
                            }
                            pos = p;
                        } else {
                            pos = mend;
                        }
                    } else {
                        prevcount = "once".to_string();
                        pos = line.len();
                    }
                }

                if let Some(path) = &pattern.concat {
                    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                        let _ = writeln!(f, "{}", line);
                    }
                    if pattern.colours.is_none() {
                        break;
                    }
                }
                if let Some(cmd) = &pattern.command {
                    let _ = Command::new("sh").arg("-c").arg(cmd).status();
                    if pattern.colours.is_none() {
                        break;
                    }
                }
                if let Some(s) = &pattern.skip {
                    skip = matches!(s.as_str(), "yes" | "1" | "true");
                    if pattern.colours.is_none() {
                        break;
                    }
                }
            }
            if m_present && currcount == "stop" {
                prevcount = "stop".to_string();
                break;
            }
        }

        if clist.is_empty() {
            prevcolour = default.clone();
        }

        let chars: Vec<char> = line.chars().collect();
        let n = chars.len();
        let mut first_char = false;
        let mut last_char = false;

        let cline: Vec<String> = if !blockflag {
            let mut cline = vec![default.clone(); n + 1];
            for (s, e, col) in &clist {
                let cs = (*s).min(n);
                let ce = (*e).min(n);
                if col == "prev" {
                    for slot in cline.iter_mut().take(ce).skip(cs) {
                        *slot = format!("{}{}", default, prevcolour);
                    }
                } else if col != "unchanged" {
                    for slot in cline.iter_mut().take(ce).skip(cs) {
                        *slot = format!("{}{}", default, col);
                    }
                }
                if *s == 0 {
                    first_char = true;
                    if col != "prev" {
                        prevcolour = col.clone();
                    }
                }
                if *e == n {
                    last_char = true;
                }
            }
            if !first_char || !last_char {
                prevcolour = default.clone();
            }
            cline
        } else {
            vec![blockcolour.clone(); n + 1]
        };

        if !skip {
            let mut nline = String::new();
            let mut clineprev = String::new();
            for i in 0..n {
                if cline[i] == clineprev {
                    nline.push(chars[i]);
                } else {
                    nline.push_str(&cline[i]);
                    nline.push(chars[i]);
                    clineprev = cline[i].clone();
                }
            }
            nline.push_str(&default);
            if writeln!(writer, "{}", nline).is_err() {
                break; // EPIPE: downstream closed
            }
        }
    }
    let _ = writer.flush();
}
