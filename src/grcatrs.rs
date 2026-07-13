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
    /// grcrs extension: streaming delta-driven colour for a numeric group.
    trend: Option<TrendSpec>,
    /// Capture group whose text keys the per-key last-value map (grcrs extension).
    key: Option<usize>,
    /// Capture group whose text is parsed as the tracked metric (grcrs extension).
    metric: Option<usize>,
}

/// Resolved colours for the three motions of a `trend=` directive. An empty
/// string means "no colour for this motion" (the group falls back to default).
struct TrendSpec {
    rising: String,
    falling: String,
    steady: String,
}

/// Choose the colour for `value` under a `trend=` directive, updating the
/// per-key last-value map. The first observation of a key has no prior value
/// and so counts as `steady`; a repeated equal value is also `steady`.
fn trend_colour(
    spec: &TrendSpec,
    last: &mut HashMap<String, f64>,
    key: String,
    value: f64,
) -> String {
    let out = match last.get(&key).copied() {
        Some(prev) if value > prev => spec.rising.clone(),
        Some(prev) if value < prev => spec.falling.clone(),
        _ => spec.steady.clone(),
    };
    last.insert(key, value);
    out
}

/// Byte-span text of capture group `idx`, or `None` if it did not participate.
fn group_text<'a>(line: &'a str, groups: &[Option<(usize, usize)>], idx: usize) -> Option<&'a str> {
    groups.get(idx).and_then(|s| *s).map(|(a, b)| &line[a..b])
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
    const KEYWORDS: [&str; 10] = [
        "regexp", "colours", "count", "command", "skip", "replace", "concat", "trend", "key",
        "metric",
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

            // grcrs extension: parse the streaming-trend directives.
            let trend = match block.get("trend") {
                Some(tv) => {
                    let mut spec = TrendSpec {
                        rising: String::new(),
                        falling: String::new(),
                        steady: String::new(),
                    };
                    for pair in tv.split(',') {
                        let (dir, colspec) = pair.split_once(':').ok_or_else(|| {
                            format!("bad trend spec, expected dir:colour: {:?}", pair)
                        })?;
                        let mut c = String::new();
                        for tok in colspec.split_whitespace() {
                            c.push_str(&get_colour(tok, table)?);
                        }
                        match dir.trim() {
                            "rising" => spec.rising = c,
                            "falling" => spec.falling = c,
                            "steady" => spec.steady = c,
                            other => return Err(format!("unknown trend direction: {}", other)),
                        }
                    }
                    Some(spec)
                }
                None => None,
            };
            let key = match block.get("key") {
                Some(kv) => Some(
                    kv.trim()
                        .parse::<usize>()
                        .map_err(|_| format!("bad key group: {}", kv))?,
                ),
                None => None,
            };
            let metric = match block.get("metric") {
                Some(mv) => Some(
                    mv.trim()
                        .parse::<usize>()
                        .map_err(|_| format!("bad metric group: {}", mv))?,
                ),
                None => None,
            };

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
                trend,
                key,
                metric,
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

/// The UTF-8 bytes of a single `char`.
fn ch_bytes(ch: char) -> Vec<u8> {
    let mut b = [0u8; 4];
    ch.encode_utf8(&mut b).as_bytes().to_vec()
}

/// Decode raw line bytes the way grcat's `surrogateescape` does, so invalid
/// bytes survive round-trip. Returns a matching string with exactly one `char`
/// per input unit — each undecodable byte becomes a single U+FFFD, matching
/// Python's one-code-point-per-byte behaviour so char indices line up — plus the
/// original source bytes for each char position, so output can re-emit invalid
/// bytes unchanged instead of the lossy replacement character.
fn decode_units(raw: &[u8]) -> (String, Vec<Vec<u8>>) {
    let mut s = String::with_capacity(raw.len());
    let mut orig: Vec<Vec<u8>> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match std::str::from_utf8(&raw[i..]) {
            Ok(valid) => {
                for ch in valid.chars() {
                    s.push(ch);
                    orig.push(ch_bytes(ch));
                }
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                let valid = std::str::from_utf8(&raw[i..i + good])
                    .expect("valid_up_to prefix is valid utf8");
                for ch in valid.chars() {
                    s.push(ch);
                    orig.push(ch_bytes(ch));
                }
                // One invalid byte → one U+FFFD unit that round-trips to it.
                s.push('\u{FFFD}');
                orig.push(vec![raw[i + good]]);
                i += good + 1;
            }
        }
    }
    (s, orig)
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
    // grcrs extension: last-seen metric value per (pattern, key) for `trend=`.
    let mut trend_last: HashMap<String, f64> = HashMap::new();

    let mut raw: Vec<u8> = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        // Strip a single trailing newline byte (mirrors grcat's readline).
        if matches!(raw.last(), Some(b'\n') | Some(b'\r')) {
            raw.pop();
        }
        // Decode leniently; command output is not guaranteed valid UTF-8. `orig`
        // keeps each char's source bytes so invalid bytes round-trip unchanged.
        let (mut line, orig) = decode_units(&raw);
        let mut line_modified = false;

        // clist: (char_start, char_end, colour_string)
        let mut clist: Vec<(usize, usize, String)> = Vec::new();
        let mut skip = false;

        for (pattern_idx, pattern) in patterns.iter().enumerate() {
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
                    // After a rewrite, `orig` no longer aligns with `line`; fall
                    // back to the decoded chars for output.
                    line_modified = true;
                }

                if let Some(cols) = &pattern.colours {
                    if currcount == "block" {
                        blockflag = true;
                        blockcolour = cols[0].clone();
                        // grcat marks the block-start as a stop so no later
                        // pattern runs on this line and prevcount becomes "stop".
                        currcount = "stop".to_string();
                        break;
                    } else if currcount == "unblock" {
                        blockflag = false;
                        blockcolour = default.clone();
                        currcount = "stop".to_string();
                    }
                    // grcrs extension: when a `trend=` directive is present,
                    // recolour the metric group by the sign of value - last[key].
                    if let (Some(spec), Some(mi)) = (&pattern.trend, pattern.metric) {
                        let mut ec = cols.clone();
                        let key = pattern
                            .key
                            .and_then(|k| group_text(&line, &groups, k))
                            .unwrap_or("")
                            .to_string();
                        if let Some(v) = group_text(&line, &groups, mi)
                            .and_then(|t| t.trim().parse::<f64>().ok())
                        {
                            let composite = format!("{}\u{0}{}", pattern_idx, key);
                            let tc = trend_colour(spec, &mut trend_last, composite, v);
                            if mi >= ec.len() {
                                ec.resize(mi + 1, cols[0].clone());
                            }
                            ec[mi] = tc;
                        }
                        add2list(&mut clist, &groups, &ec, &line);
                    } else {
                        add2list(&mut clist, &groups, cols, &line);
                    }
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
            let mut nline: Vec<u8> = Vec::new();
            let mut clineprev: &str = "";
            let mut tmp = [0u8; 4];
            for i in 0..n {
                if cline[i].as_str() != clineprev {
                    nline.extend_from_slice(cline[i].as_bytes());
                    clineprev = cline[i].as_str();
                }
                // Emit the char's original bytes so invalid UTF-8 survives; after
                // a `replace` rewrite, `orig` is stale so fall back to the char.
                if line_modified {
                    nline.extend_from_slice(chars[i].encode_utf8(&mut tmp).as_bytes());
                } else {
                    nline.extend_from_slice(&orig[i]);
                }
            }
            nline.extend_from_slice(default.as_bytes());
            nline.push(b'\n');
            if writer.write_all(&nline).is_err() {
                break; // EPIPE: downstream closed
            }
        }
    }
    let _ = writer.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_octal_is_python_eval() {
        // grc configs write `"\033[38;5;208m"`; Python eval decodes the octal.
        assert_eq!(unescape(r"\033[38;5;208m"), "\x1b[38;5;208m");
        assert_eq!(unescape(r"\0"), "\0");
    }

    #[test]
    fn unescape_hex_and_named_escapes() {
        assert_eq!(unescape(r"\x1b[0m"), "\x1b[0m");
        assert_eq!(unescape(r"\n\t\r"), "\n\t\r");
        assert_eq!(unescape(r"a\\b"), "a\\b");
        // an unknown escape keeps the backslash, like a raw byte pass-through
        assert_eq!(unescape(r"\q"), "\\q");
    }

    #[test]
    fn translate_regex_angle_brackets_become_literals() {
        // Python `re` has no \< / \> boundary; they are literal characters.
        assert_eq!(translate_regex(r"^\>(.*)"), "^>(.*)");
        assert_eq!(translate_regex(r"\<none\>"), "<none>");
        assert_eq!(
            translate_regex(r"(\<?[Nn]one\>?|null)"),
            "(<?[Nn]one>?|null)"
        );
        // Other escapes are left untouched.
        assert_eq!(translate_regex(r"\d+\.\s(?=\sMar)"), r"\d+\.\s(?=\sMar)");
    }

    #[test]
    fn convert_backrefs_to_dollar_form() {
        assert_eq!(convert_backrefs(r"TIMEOUT \1"), "TIMEOUT ${1}");
        assert_eq!(convert_backrefs(r"\1h\2m\3s"), "${1}h${2}m${3}s");
        // A literal dollar must be doubled so it is not read as a group ref.
        assert_eq!(convert_backrefs("cost $5"), "cost $$5");
    }

    #[test]
    fn get_colour_named_quoted_and_invalid() {
        let t = colour_table();
        assert_eq!(get_colour("red", &t).unwrap(), "\x1b[31m");
        assert_eq!(get_colour("bold", &t).unwrap(), "\x1b[1m");
        assert_eq!(get_colour("previous", &t).unwrap(), "prev");
        assert_eq!(get_colour("unchanged", &t).unwrap(), "unchanged");
        assert_eq!(get_colour(r#""\033[1m""#, &t).unwrap(), "\x1b[1m");
        assert!(get_colour("notacolour", &t).is_err());
    }

    #[test]
    fn byte_to_char_handles_multibyte_and_clamps() {
        let s = "aéb"; // 'é' is two bytes (0xC3 0xA9)
        assert_eq!(byte_to_char(s, 0), 0);
        assert_eq!(byte_to_char(s, 1), 1); // start of 'é'
        assert_eq!(byte_to_char(s, 3), 2); // first byte after 'é'
        assert_eq!(byte_to_char(s, 999), 3); // clamps to char count
    }

    #[test]
    fn parse_config_resolves_colours_count_and_regex() {
        let t = colour_table();
        let pats = parse_config("regexp=foo\ncolours=red bold\n", &t).unwrap();
        assert_eq!(pats.len(), 1);
        assert_eq!(pats[0].count, "more");
        // Multiple whitespace-separated colours in one group concatenate.
        assert_eq!(
            pats[0].colours.as_ref().unwrap(),
            &vec!["\x1b[31m\x1b[1m".to_string()]
        );
    }

    #[test]
    fn parse_config_splits_comma_groups() {
        let t = colour_table();
        let pats = parse_config("regexp=(a)(b)\ncolours=red,green,blue\ncount=stop\n", &t).unwrap();
        let cols = pats[0].colours.as_ref().unwrap();
        assert_eq!(
            cols,
            &vec![
                "\x1b[31m".to_string(),
                "\x1b[32m".to_string(),
                "\x1b[34m".to_string()
            ]
        );
        assert_eq!(pats[0].count, "stop");
    }

    #[test]
    fn parse_config_accepts_lookahead_and_us_spelling() {
        // Lookahead is why grcrs uses fancy-regex; `color`/`colours` are aliases.
        let t = colour_table();
        let pats = parse_config("regexp=\\d+(?=\\sMar)\ncolor=green\n", &t).unwrap();
        assert_eq!(pats.len(), 1);
        assert_eq!(
            pats[0].colours.as_ref().unwrap(),
            &vec!["\x1b[32m".to_string()]
        );
    }

    #[test]
    fn parse_config_skips_blocks_without_regexp() {
        let t = colour_table();
        // A comment-only block contributes no pattern.
        let pats = parse_config("# just a comment\ncolours=red\n", &t).unwrap();
        assert!(pats.is_empty());
    }

    #[test]
    fn parse_config_rejects_bad_keyword() {
        let t = colour_table();
        assert!(parse_config("bogus=1\n", &t).is_err());
    }

    #[test]
    fn parse_config_captures_command_skip_concat_replace() {
        let t = colour_table();
        let cfg =
            "regexp=(\\d+)\nreplace=N \\1\ncommand=true\nskip=yes\nconcat=/tmp/x\ncolours=red\n";
        let pats = parse_config(cfg, &t).unwrap();
        assert_eq!(pats.len(), 1);
        let p = &pats[0];
        assert_eq!(p.command.as_deref(), Some("true"));
        assert_eq!(p.skip.as_deref(), Some("yes"));
        assert_eq!(p.concat.as_deref(), Some("/tmp/x"));
        // replace backrefs are rewritten to the ${N} form at parse time.
        assert_eq!(p.replace.as_deref(), Some("N ${1}"));
    }

    #[test]
    fn parse_config_multiple_blocks_split_on_separator() {
        let t = colour_table();
        // A line not starting with '#', a letter, or blank ends a block.
        let cfg = "regexp=a\ncolours=red\n======\nregexp=b\ncolours=blue\ncount=stop\n";
        let pats = parse_config(cfg, &t).unwrap();
        assert_eq!(pats.len(), 2);
        assert_eq!(pats[0].count, "more");
        assert_eq!(pats[1].count, "stop");
        assert_eq!(
            pats[1].colours.as_ref().unwrap(),
            &vec!["\x1b[34m".to_string()]
        );
    }

    #[test]
    fn parse_config_count_defaults_to_more() {
        let t = colour_table();
        let pats = parse_config("regexp=z\ncolours=cyan\n", &t).unwrap();
        assert_eq!(pats[0].count, "more");
    }

    #[test]
    fn parse_config_blank_and_comment_lines_ignored_inside_block() {
        let t = colour_table();
        let cfg = "# comment\nregexp=q\n# mid comment\ncolours=green\n";
        let pats = parse_config(cfg, &t).unwrap();
        assert_eq!(pats.len(), 1);
        assert_eq!(
            pats[0].colours.as_ref().unwrap(),
            &vec!["\x1b[32m".to_string()]
        );
    }

    #[test]
    fn parse_config_missing_equals_is_error() {
        let t = colour_table();
        assert!(parse_config("regexp\n", &t).is_err());
    }

    #[test]
    fn convert_backrefs_multi_digit_group() {
        // Two-digit backrefs must consume both digits.
        assert_eq!(convert_backrefs(r"\12"), "${12}");
        assert_eq!(convert_backrefs(r"\1\2"), "${1}${2}");
    }

    #[test]
    fn unescape_octal_stops_after_three_digits() {
        // Python octal escapes are at most three digits: \0111 == \011 + '1'.
        assert_eq!(unescape(r"\0111"), "\t1");
        // \8 is not an octal digit, so the backslash is kept.
        assert_eq!(unescape(r"\8"), "\\8");
    }

    #[test]
    fn unescape_hex_caps_at_two_digits_and_bell() {
        // A `\xNN` escape consumes at most two hex digits; trailing digits are
        // literal. `\x41` == 'A', so `\x4142` == "A42".
        assert_eq!(unescape(r"\x4142"), "A42");
        // A non-hex char after the two digits is passed through unchanged.
        assert_eq!(unescape(r"\x41Z"), "AZ");
        // Bell escape via \a maps to BEL.
        assert_eq!(unescape(r"\a"), "\x07");
    }

    #[test]
    fn colour_table_has_bright_and_background_codes() {
        let t = colour_table();
        assert_eq!(t["bright_red"], "\x1b[31;91m");
        assert_eq!(t["on_bright_white"], "\x1b[47;107m");
        assert_eq!(t["none"], "");
        assert_eq!(t["beep"], "\x07");
    }

    #[test]
    fn get_colour_concatenation_via_parse_config() {
        // A colour group with several tokens concatenates their escapes.
        let t = colour_table();
        let pats = parse_config("regexp=x\ncolours=on_blue bold white\n", &t).unwrap();
        assert_eq!(
            pats[0].colours.as_ref().unwrap(),
            &vec!["\x1b[44m\x1b[1m\x1b[37m".to_string()]
        );
    }

    #[test]
    fn decode_units_maps_each_invalid_byte_to_one_roundtrip_unit() {
        // Valid text decodes 1:1; each invalid byte becomes one U+FFFD unit whose
        // original byte is preserved, matching Python's per-byte surrogateescape.
        let (s, orig) = decode_units(b"a\xffb\xc3\xa9");
        // chars: 'a', U+FFFD, 'b', 'é'  →  four units.
        assert_eq!(s.chars().count(), 4);
        assert_eq!(s, "a\u{FFFD}bé");
        assert_eq!(orig.len(), 4);
        assert_eq!(orig[0], b"a");
        assert_eq!(orig[1], vec![0xff]); // the raw invalid byte, not U+FFFD's bytes
        assert_eq!(orig[2], b"b");
        assert_eq!(orig[3], vec![0xc3, 0xa9]); // 'é' keeps its two source bytes
    }

    #[test]
    fn decode_units_splits_multibyte_invalid_run_per_byte() {
        // Two consecutive invalid bytes yield two separate units (Python emits one
        // surrogate per byte), so char indices stay aligned with the reference.
        let (s, orig) = decode_units(b"\xff\xfe");
        assert_eq!(s, "\u{FFFD}\u{FFFD}");
        assert_eq!(orig, vec![vec![0xffu8], vec![0xfeu8]]);
    }

    #[test]
    fn translate_regex_preserves_backslash_before_multibyte() {
        // A backslash followed by a non-<> char (here multibyte) is untouched.
        assert_eq!(translate_regex(r"\é"), r"\é");
    }

    #[test]
    fn trend_colour_picks_by_delta_sign() {
        // rising→A, falling→B, steady (equal, and first observation)→dim.
        let spec = TrendSpec {
            rising: "A".to_string(),
            falling: "B".to_string(),
            steady: "D".to_string(),
        };
        let mut last = HashMap::new();
        assert_eq!(trend_colour(&spec, &mut last, "k".to_string(), 10.0), "D"); // first: steady
        assert_eq!(trend_colour(&spec, &mut last, "k".to_string(), 12.0), "A"); // rising
        assert_eq!(trend_colour(&spec, &mut last, "k".to_string(), 4.0), "B"); // falling
        assert_eq!(trend_colour(&spec, &mut last, "k".to_string(), 4.0), "D"); // equal: steady
    }

    #[test]
    fn trend_colour_is_isolated_per_key() {
        let spec = TrendSpec {
            rising: "A".to_string(),
            falling: "B".to_string(),
            steady: "D".to_string(),
        };
        let mut last = HashMap::new();
        assert_eq!(trend_colour(&spec, &mut last, "a".to_string(), 100.0), "D");
        assert_eq!(trend_colour(&spec, &mut last, "b".to_string(), 1.0), "D");
        // key "b" rises independently of key "a"'s much larger last value.
        assert_eq!(trend_colour(&spec, &mut last, "b".to_string(), 2.0), "A");
        // key "a" falls independently of key "b".
        assert_eq!(trend_colour(&spec, &mut last, "a".to_string(), 50.0), "B");
    }

    #[test]
    fn parse_config_trend_key_metric() {
        let t = colour_table();
        let cfg = "regexp=(\\w+)\\s+(\\d+)\ncolours=default,default\ntrend=rising:green,falling:red,steady:dark\nkey=1\nmetric=2\n";
        let pats = parse_config(cfg, &t).unwrap();
        assert_eq!(pats.len(), 1);
        let p = &pats[0];
        assert_eq!(p.key, Some(1));
        assert_eq!(p.metric, Some(2));
        let spec = p.trend.as_ref().unwrap();
        assert_eq!(spec.rising, "\x1b[32m");
        assert_eq!(spec.falling, "\x1b[31m");
        assert_eq!(spec.steady, "\x1b[2m");
    }
}
