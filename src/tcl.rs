// SPDX-License-Identifier: Apache-2.0
//! The part of Tcl a UPF file is written in, and the argument parsing every UPF command shares.
//!
//! A UPF file is a Tcl script: reading it SOURCES it, so it could hold any Tcl. This reader takes
//! the subset UPF files are written in: one command per line or `;`, `#` comments, `\`-newline
//! continuations, and words that are bare, `"quoted"` or `{braced}`. A `$variable` or `[command]`
//! substitution is not evaluated: it is refused, never read as literal text.
//!
//! The argument parsing is the reference's shared Tcl, transcribed: [`parse_key_args`] (exact key,
//! exact flag, then the first key or flag the argument is a PREFIX of), [`parse_list_args`]
//! (repeatable options, collected in order before the keys are parsed), and the positional-count
//! checks.

use std::collections::{BTreeMap, BTreeSet};

/// A failure while reading: an error the reference also stops on (`Error`), or Tcl this reader
/// does not evaluate (`Refused`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The reference stops here too: `[ERROR <TOOL>-<id>] <message>`.
    Error { tool: &'static str, id: u32, msg: String },
    /// Valid Tcl, perhaps, but outside what this reader evaluates.
    Refused(String),
    /// The database's own error (`ODB-0154: ...`), which the database's logger has already
    /// printed: it stops the read as an error, and is not printed a second time.
    Database { tool: String, id: u32, msg: String },
}

impl Stop {
    pub fn error(tool: &'static str, id: u32, msg: impl Into<String>) -> Stop {
        Stop::Error { tool, id, msg: msg.into() }
    }
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::Error { tool, id, msg } => write!(f, "[ERROR {tool}-{id:04}] {msg}"),
            Stop::Refused(m) => write!(f, "refused: {m}"),
            Stop::Database { tool, id, msg } => write!(f, "[ERROR {tool}-{id:04}] {msg}"),
        }
    }
}

/// One command of a script: its words after substitution, and the line it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub line: usize,
    pub words: Vec<String>,
}

/// Split a script into commands. Braced words are kept verbatim (a `\`-newline in them becomes one
/// space, as in Tcl); bare and quoted words get backslash substitution.
pub fn parse_script(src: &str) -> Result<Vec<Command>, Stop> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1;
    let mut out = Vec::new();
    while i < c.len() {
        // Skip separators between commands.
        while i < c.len() && matches!(c[i], ' ' | '\t' | '\n' | '\r' | ';') {
            if c[i] == '\n' {
                line += 1;
            }
            i += 1;
        }
        if i >= c.len() {
            break;
        }
        if c[i] == '#' {
            // A comment runs to the end of the line; a `\`-newline continues it.
            while i < c.len() && c[i] != '\n' {
                if c[i] == '\\' && c.get(i + 1) == Some(&'\n') {
                    line += 1;
                    i += 1;
                }
                i += 1;
            }
            continue;
        }
        let start = line;
        let mut words = Vec::new();
        loop {
            // Whitespace between words; a `\`-newline is whitespace too.
            while i < c.len() {
                if matches!(c[i], ' ' | '\t' | '\r') {
                    i += 1;
                } else if c[i] == '\\' && c.get(i + 1) == Some(&'\n') {
                    line += 1;
                    i += 2;
                } else {
                    break;
                }
            }
            if i >= c.len() || c[i] == '\n' || c[i] == ';' {
                break;
            }
            let (w, next, nl) = word(&c, i, line)?;
            words.push(w);
            i = next;
            line += nl;
        }
        out.push(Command { line: start, words });
    }
    Ok(out)
}

/// One word starting at `i`: `(text, index after it, newlines consumed)`.
fn word(c: &[char], mut i: usize, line: usize) -> Result<(String, usize, usize), Stop> {
    let mut nl = 0;
    let mut s = String::new();
    match c[i] {
        '{' => {
            let mut depth = 1;
            i += 1;
            while i < c.len() {
                match c[i] {
                    '\\' if c.get(i + 1) == Some(&'\n') => {
                        // `\`-newline and the whitespace after it: one space.
                        nl += 1;
                        i += 2;
                        while i < c.len() && matches!(c[i], ' ' | '\t') {
                            i += 1;
                        }
                        s.push(' ');
                        continue;
                    }
                    '\\' if i + 1 < c.len() => {
                        s.push('\\');
                        s.push(c[i + 1]);
                        i += 2;
                        continue;
                    }
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            if i < c.len() && !matches!(c[i], ' ' | '\t' | '\r' | '\n' | ';') {
                                return Err(Stop::Refused(format!("line {line}: extra characters after close-brace")));
                            }
                            return Ok((s, i, nl));
                        }
                    }
                    '\n' => nl += 1,
                    _ => {}
                }
                s.push(c[i]);
                i += 1;
            }
            Err(Stop::Refused(format!("line {line}: missing close-brace")))
        }
        '"' => {
            i += 1;
            while i < c.len() && c[i] != '"' {
                match c[i] {
                    '$' | '[' => return Err(substitution(line, c[i])),
                    '\\' => {
                        let (ch, used, n) = backslash(c, i);
                        s.push_str(&ch);
                        i += used;
                        nl += n;
                    }
                    ch => {
                        if ch == '\n' {
                            nl += 1;
                        }
                        s.push(ch);
                        i += 1;
                    }
                }
            }
            if i >= c.len() {
                return Err(Stop::Refused(format!("line {line}: missing \"")));
            }
            i += 1;
            if i < c.len() && !matches!(c[i], ' ' | '\t' | '\r' | '\n' | ';') {
                return Err(Stop::Refused(format!("line {line}: extra characters after close-quote")));
            }
            Ok((s, i, nl))
        }
        _ => {
            while i < c.len() && !matches!(c[i], ' ' | '\t' | '\r' | '\n' | ';') {
                match c[i] {
                    '$' | '[' => return Err(substitution(line, c[i])),
                    '\\' if c.get(i + 1) == Some(&'\n') => break,
                    '\\' => {
                        let (ch, used, n) = backslash(c, i);
                        s.push_str(&ch);
                        i += used;
                        nl += n;
                    }
                    ch => {
                        s.push(ch);
                        i += 1;
                    }
                }
            }
            Ok((s, i, nl))
        }
    }
}

fn substitution(line: usize, ch: char) -> Stop {
    let what = if ch == '$' { "a variable substitution ($)" } else { "a command substitution ([...])" };
    Stop::Refused(format!("line {line}: {what} is not evaluated by this reader"))
}

/// Tcl backslash substitution at `c[i] == '\\'`: `(text, chars used, newlines consumed)`.
fn backslash(c: &[char], i: usize) -> (String, usize, usize) {
    match c.get(i + 1) {
        None => ("\\".into(), 1, 0),
        Some('n') => ("\n".into(), 2, 0),
        Some('t') => ("\t".into(), 2, 0),
        Some('r') => ("\r".into(), 2, 0),
        Some('\n') => {
            let mut j = i + 2;
            while j < c.len() && matches!(c[j], ' ' | '\t') {
                j += 1;
            }
            (" ".into(), j - i, 1)
        }
        Some(&ch) => (ch.to_string(), 2, 0),
    }
}

/// A Tcl list's elements (`lindex`, `llength`, `foreach` all read a value this way).
pub fn split_list(s: &str) -> Result<Vec<String>, Stop> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    loop {
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        if i >= c.len() {
            return Ok(out);
        }
        let el = match c[i] {
            '{' => {
                let mut depth = 1;
                let mut e = String::new();
                i += 1;
                while i < c.len() {
                    match c[i] {
                        '\\' if i + 1 < c.len() => {
                            e.push(c[i]);
                            e.push(c[i + 1]);
                            i += 2;
                            continue;
                        }
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    e.push(c[i]);
                    i += 1;
                }
                if i >= c.len() {
                    return Err(Stop::Refused(format!("unmatched open brace in list {s:?}")));
                }
                i += 1;
                if i < c.len() && !c[i].is_whitespace() {
                    return Err(Stop::Refused(format!("list element in braces followed by other text: {s:?}")));
                }
                e
            }
            '"' => {
                let mut e = String::new();
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' {
                        let (ch, used, _) = backslash(&c, i);
                        e.push_str(&ch);
                        i += used;
                    } else {
                        e.push(c[i]);
                        i += 1;
                    }
                }
                if i >= c.len() {
                    return Err(Stop::Refused(format!("unmatched open quote in list {s:?}")));
                }
                i += 1;
                e
            }
            _ => {
                let mut e = String::new();
                while i < c.len() && !c[i].is_whitespace() {
                    if c[i] == '\\' {
                        let (ch, used, _) = backslash(&c, i);
                        e.push_str(&ch);
                        i += used;
                    } else {
                        e.push(c[i]);
                        i += 1;
                    }
                }
                e
            }
        };
        out.push(el);
    }
}

/// `sta::is_keyword_arg`: at least two characters, a `-`, then a letter.
pub fn is_keyword_arg(a: &str) -> bool {
    let mut it = a.chars();
    a.chars().count() >= 2 && it.next() == Some('-') && it.next().is_some_and(|c| c.is_alphabetic())
}

/// The result of [`parse_key_args`]: the positional arguments left, each key's value (a key given
/// twice keeps the last), and the flags present.
#[derive(Debug, Default)]
pub struct KeyArgs {
    pub args: Vec<String>,
    pub keys: BTreeMap<String, String>,
    pub flags: BTreeSet<String>,
}

/// `sta::parse_key_args`, unknown keys an error: an exact key, else an exact flag, else the FIRST
/// key (in `keys` order) the argument is a prefix of, else the first such flag.
pub fn parse_key_args(cmd: &str, args: Vec<String>, keys: &[&str], flags: &[&str]) -> Result<KeyArgs, Stop> {
    let mut r = KeyArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if is_keyword_arg(arg) {
            let key = keys.iter().find(|k| *k == arg).map(|k| (k, 560));
            let key = key.or_else(|| {
                (!flags.contains(&arg.as_str())).then(|| keys.iter().find(|k| k.starts_with(arg.as_str())).map(|k| (k, 561))).flatten()
            });
            if let Some((k, id)) = key {
                if i + 1 >= args.len() {
                    return Err(Stop::error("STA", id, format!("{cmd} {k} missing value.")));
                }
                r.keys.insert(k.to_string(), args[i + 1].clone());
                i += 2;
                continue;
            }
            if let Some(f) = flags.iter().find(|f| *f == arg).or_else(|| flags.iter().find(|f| f.starts_with(arg.as_str()))) {
                r.flags.insert(f.to_string());
            } else {
                return Err(Stop::error("STA", 562, format!("{cmd} {arg} is not a known keyword or flag.")));
            }
        } else {
            r.args.push(arg.clone());
        }
        i += 1;
    }
    Ok(r)
}

/// Each repeatable option's values, in the order given.
pub type ListArgs = BTreeMap<String, Vec<String>>;

/// `ord::parse_list_args`: for each option in turn, take every EXACT `option value` pair out of the
/// arguments, values in order. Runs before [`parse_key_args`], which then sees the rest.
pub fn parse_list_args(
    cmd: &str,
    mut args: Vec<String>,
    opts: &[&str],
) -> Result<(Vec<String>, ListArgs), Stop> {
    let mut lists = BTreeMap::new();
    for opt in opts {
        let mut rest = Vec::new();
        let mut vals = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if is_keyword_arg(a) && a == opt {
                i += 1;
                if i == args.len() {
                    return Err(Stop::error("ORD", 560, format!("{cmd} {opt} missing value.")));
                }
                vals.push(args[i].clone());
            } else {
                rest.push(a.clone());
            }
            i += 1;
        }
        lists.insert(opt.to_string(), vals);
        args = rest;
    }
    Ok((args, lists))
}

/// `sta::check_argc_eq1`.
pub fn check_argc_eq1(cmd: &str, args: &[String]) -> Result<(), Stop> {
    if args.len() != 1 {
        return Err(Stop::error("STA", 565, format!("{cmd} requires one positional argument.")));
    }
    Ok(())
}

/// `sta::check_argc_eq0`.
pub fn check_argc_eq0(cmd: &str, args: &[String]) -> Result<(), Stop> {
    if !args.is_empty() {
        return Err(Stop::error("STA", 564, format!("{cmd} positional arguments not supported.")));
    }
    Ok(())
}

/// Tcl's `string is double`: a whole string that reads as a floating-point number (surrounding
/// whitespace allowed).
pub fn is_double(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && t.parse::<f64>().is_ok() && !t.eq_ignore_ascii_case("inf") && !t.eq_ignore_ascii_case("nan")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// A UPF command split over lines by `\`, with braced lists kept verbatim and comments dropped.
    #[test]
    fn a_continued_command_is_one_command() {
        let src = "# header\ncreate_power_switch psw \\\n  -domain PD \\\n  -on_state {ON vin {!sleep}}\nset_isolation iso -domain PD ; # trailing\n";
        let cmds = parse_script(src).unwrap();
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        assert_eq!(cmds[0].words, s(&["create_power_switch", "psw", "-domain", "PD", "-on_state", "ON vin {!sleep}"]));
        assert_eq!(cmds[0].line, 2);
        assert_eq!(cmds[1].words, s(&["set_isolation", "iso", "-domain", "PD"]));
        assert_eq!(cmds[1].line, 5);
    }

    /// Substitution is refused, never read as text: `$v` and `[cmd]` are not names.
    #[test]
    fn substitution_is_refused() {
        for src in ["create_power_domain $pd", "create_power_domain [get_pd]", "create_logic_port \"a$b\""] {
            assert!(matches!(parse_script(src), Err(Stop::Refused(_))), "{src}");
        }
        // Inside braces nothing is substituted, so it is fine.
        assert_eq!(parse_script("x {$a [b]}").unwrap()[0].words[1], "$a [b]");
    }

    /// A list element in braces keeps its own braces' content; `{}` is one empty element.
    #[test]
    fn list_elements_as_tcl_reads_them() {
        assert_eq!(split_list("vin1 {} {a b}").unwrap(), s(&["vin1", "", "a b"]));
        assert_eq!(split_list("ON vin {!sleep}").unwrap(), s(&["ON", "vin", "!sleep"]));
        assert_eq!(split_list("  ").unwrap(), Vec::<String>::new());
    }

    /// The reference's key rules: exact match first, then the first key the argument is a PREFIX
    /// of; a flag likewise; anything else is STA-0562.
    #[test]
    fn keys_match_exactly_then_by_prefix() {
        let r = parse_key_args("c", s(&["-dom", "PD", "-up", "iso"]), &["-domain", "-applies_to"], &["-update"]).unwrap();
        assert_eq!(r.keys["-domain"], "PD");
        assert!(r.flags.contains("-update"));
        assert_eq!(r.args, s(&["iso"]));
        let e = parse_key_args("c", s(&["-nope", "x"]), &["-domain"], &[]).unwrap_err();
        assert_eq!(e.to_string(), "[ERROR STA-0562] c -nope is not a known keyword or flag.");
        let e = parse_key_args("c", s(&["-domain"]), &["-domain"], &[]).unwrap_err();
        assert_eq!(e.to_string(), "[ERROR STA-0560] c -domain missing value.");
        // A key given twice keeps the last value; `-1` is not a keyword (a digit follows the dash).
        let r = parse_key_args("c", s(&["-domain", "A", "-domain", "B", "-1"]), &["-domain"], &[]).unwrap();
        assert_eq!((r.keys["-domain"].as_str(), r.args.clone()), ("B", s(&["-1"])));
    }

    /// Repeatable options are collected in order, exact spellings only, before the keys.
    #[test]
    fn list_args_collect_every_exact_occurrence() {
        let (rest, l) = parse_list_args("c", s(&["p", "-control_port", "a", "-domain", "D", "-control_port", "b", "-control"]), &["-control_port"]).unwrap();
        assert_eq!(l["-control_port"], s(&["a", "b"]));
        assert_eq!(rest, s(&["p", "-domain", "D", "-control"]));
    }
}
