//! End-to-end tests driving the built `grcat` and `grc` binaries.
//!
//! Each test writes a self-contained grcat config to a temp file and pipes
//! known input through the binary, asserting the exact ANSI byte stream. No
//! system grc install or fixed paths are required, so these run in headless CI.
//!
//! Expected outputs were derived from grc's documented colourising algorithm
//! and cross-checked byte-for-byte against the reference Python `grcat`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Write `conf` to a unique temp file and return its absolute path. grcat's
/// search path starts with "", so an absolute path resolves directly.
fn write_conf(conf: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("grcrs_test_{}_{}.conf", std::process::id(), n));
    std::fs::write(&path, conf).unwrap();
    path
}

/// Run `grcat CONF` with `input` on stdin and return its stdout as a string.
fn grcat(conf: &str, input: &str) -> String {
    let path = write_conf(conf);
    let mut child = Command::new(env!("CARGO_BIN_EXE_grcat"))
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    std::fs::remove_file(&path).ok();
    assert!(out.status.success(), "grcat exited with failure");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn basic_colour_wraps_only_the_match() {
    let out = grcat("regexp=foo\ncolours=red\n", "a foo b\n");
    assert_eq!(out, "\x1b[0ma \x1b[0m\x1b[31mfoo\x1b[0m b\x1b[0m\n");
}

#[test]
fn lookahead_regex_colours_the_number() {
    // Lookahead `(?=...)` is unsupported by the plain regex crate — this proves
    // grcrs's fancy-regex backend handles the real grc config dialect.
    let out = grcat("regexp=\\d+(?=\\sMar)\ncolours=green\n", "size 344 Mar\n");
    assert_eq!(out, "\x1b[0msize \x1b[0m\x1b[32m344\x1b[0m Mar\x1b[0m\n");
}

#[test]
fn per_group_colours_layer_in_order() {
    let out = grcat("regexp=(a)(b)\ncolours=red,green,blue\n", "ab\n");
    // group0 (red, whole) then group1 (green, 'a') then group2 (blue, 'b')
    assert_eq!(out, "\x1b[0m\x1b[32ma\x1b[0m\x1b[34mb\x1b[0m\n");
}

#[test]
fn unchanged_colour_leaves_group_untouched() {
    // group0 "unchanged" paints nothing; only group1 gets red.
    let out = grcat("regexp=(ERR)(OR)\ncolours=unchanged,red\n", "ERROR\n");
    assert_eq!(out, "\x1b[0m\x1b[31mERR\x1b[0mOR\x1b[0m\n");
}

#[test]
fn count_stop_halts_further_patterns() {
    let conf = "regexp=X\ncolours=red\ncount=stop\n======\nregexp=X\ncolours=blue\n";
    // The stop on the first pattern prevents the blue pattern from running.
    assert_eq!(grcat(conf, "X\n"), "\x1b[0m\x1b[31mX\x1b[0m\n");
}

#[test]
fn block_state_carries_across_lines() {
    let conf = "regexp=START\ncolours=italic black\ncount=block\n\
                ======\nregexp=^END\ncolours=default\ncount=unblock\n";
    let out = grcat(conf, "before\nSTART here\ninside block\nEND now\nafter\n");
    let expected = concat!(
        "\x1b[0mbefore\x1b[0m\n",
        "\x1b[3m\x1b[30mSTART here\x1b[0m\n",
        "\x1b[3m\x1b[30minside block\x1b[0m\n",
        "\x1b[0m\x1b[0mEND\x1b[0m now\x1b[0m\n",
        "\x1b[0mafter\x1b[0m\n",
    );
    assert_eq!(out, expected);
}

#[test]
fn replace_rewrites_then_colours() {
    // `\1` backref in the replacement is honoured, then the match is coloured.
    let out = grcat(
        "regexp=.*seq=(\\d+) end\nreplace=SEQ \\1\ncolours=red\n",
        "x seq=42 end\n",
    );
    assert_eq!(out, "\x1b[0m\x1b[31mSEQ 42\x1b[0m\n");
}

#[test]
fn quoted_colour_literal_is_decoded() {
    // A `"\033[...m"` literal is octal-decoded like grcat's Python eval.
    let out = grcat("regexp=hot\ncolours=\"\\033[38;5;208m\"\n", "hot dog\n");
    assert_eq!(out, "\x1b[0m\x1b[38;5;208mhot\x1b[0m dog\x1b[0m\n");
}

#[test]
fn angle_escapes_match_as_literals() {
    // `^\>` matches a leading '>' (Python literal), not a word boundary.
    let out = grcat("regexp=^\\>(.*)\ncolours=bold green\n", "> quoted\nplain\n");
    assert_eq!(
        out,
        "\x1b[0m\x1b[1m\x1b[32m> quoted\x1b[0m\n\x1b[0mplain\x1b[0m\n"
    );
}

#[test]
fn skip_suppresses_matching_lines() {
    let out = grcat(
        "regexp=secret\ncolours=red\nskip=yes\n",
        "has secret\nkeep me\n",
    );
    assert_eq!(out, "\x1b[0mkeep me\x1b[0m\n");
}

#[test]
fn grcat_missing_config_errors() {
    let child = Command::new(env!("CARGO_BIN_EXE_grcat"))
        .arg("definitely_not_a_real_conf_name_zzz")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));
}

/// Run the full `grc` wrapper: it should run the command, pipe its stdout
/// through grcat, and colour the output. Uses an absolute `-c` config so no
/// system grc.conf is consulted, and `sh -c` so the test needs no fixtures.
#[test]
fn grc_wrapper_pipes_command_through_grcat() {
    let path = write_conf("regexp=foo\ncolours=red\n");
    let out = Command::new(env!("CARGO_BIN_EXE_grc"))
        .arg("-c")
        .arg(&path)
        .arg("--colour=on")
        .arg("sh")
        .arg("-c")
        .arg("printf 'a foo b\\n'")
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "\x1b[0ma \x1b[0m\x1b[31mfoo\x1b[0m b\x1b[0m\n"
    );
}

#[test]
fn grc_propagates_exit_status() {
    let path = write_conf("regexp=x\ncolours=red\n");
    let status = Command::new(env!("CARGO_BIN_EXE_grc"))
        .arg("-c")
        .arg(&path)
        .arg("--colour=on")
        .arg("sh")
        .arg("-c")
        .arg("exit 42")
        .status()
        .unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(status.code(), Some(42));
}
