// SPDX-License-Identifier: Apache-2.0
//! `vyges-upf` — power intent (UPF) for designs with several power domains: domains, isolation, level shifters, power switches.
//!
//! ## Exit codes
//!
//! ⛔ **"found something" and "could not run" never share a code.** A CI gate that cannot tell a
//! broken tool from a bad design will eventually read a missing engine as a clean run.
//!
//! | code | meaning |
//! | --- | --- |
//! | 0 | ok |
//! | 1 | a finding (the check ran and failed) |
//! | 2 | usage, or input that could not be read |
//! | 3 | the command is NOT IMPLEMENTED in this build |

use std::process::ExitCode;

const USAGE: &str = "\
vyges physical upf — power intent (UPF) for designs with several power domains: domains, isolation, level shifters, power switches

USAGE:
  vyges physical upf run    <design.odb> <intent.upf> -o OUT.odb [--trace]
                                                    read the power intent into the database
  vyges physical upf write  <design.odb> [-o FILE]  write the database's power intent as UPF
  vyges physical upf eval   <design.odb> -o OUT.odb [--liberty FILE]... [--trace]
                                                    make the power intent physical: a fence region
                                                    and group per domain, the domain hierarchy,
                                                    isolation cells and level shifters between domains
  vyges physical upf --describe
  vyges physical upf --help
  vyges physical upf --version

OPTIONS:
  -o FILE               write the report (or the output database) to FILE
  --json                machine-readable output instead of the text report
  -q, --quiet           suppress non-essential output
  -v, --verbose         extra detail on stderr
  --liberty FILE        eval: a Liberty library (repeatable, in read order): inverters, cell
                        areas and isolation pins for the cells eval inserts
  --trace               run: one VYG-UPF line per database-layer call, in call order;
                        eval: one VYGEV line per database write, in the order made
  --describe            print a machine-readable JSON description of the command
  --bug-report          file a bug (central: vyges/community)
  --feature-request     request a feature (central)

EXIT STATUS:
  0  ok
  1  a finding — the check ran and failed
  2  usage or unreadable input
  3  refused — this build does not implement the command yet
";

/// The commands this engine will answer. One not yet built is refused (exit 3).
pub const COMMANDS: &[&str] = &["run", "write", "eval"];

const DESCRIBE: &str = r#"{
  "schema": "vyges-tool-descriptor/1.1",
  "name": "upf",
  "summary": "Power intent (UPF) for designs with several power domains: domains, isolation, level shifters, power switches.",
  "maturity": "discovered",
  "openroad_pin": "@OPENROAD_PIN@",
  "provenance_limitations": [
    "input_hash covers the argument vector, not the content of the database it names.",
    "`write` writes the power intent already in the database: power domains, logic ports, power switches with their cell map, isolation and level-shifter strategies, each section in the database's creation order. It matches the reference's own UPF writer line for line, after the first line (which names the writing tool), on 6 of 6 cases: every UPF in the reference's regression data and one written to reach what those do not (objects declared out of name order, a switch state with no supply port, an acknowledge with no net, a sense with no signal, thresholds at an exact binary tie).",
    "A power switch with no cell map is written without `map_power_switch`; the reference's writer stops on one.",
    "`run` reads a UPF file into the database as the reference's `read_upf` does: the ten UPF commands (create_power_domain, create_logic_port, create_power_switch, set_isolation, use_interface_cell, set_domain_area, map_power_switch, set_level_shifter, set_domain_voltage, set_level_shifter_cell) with the reference's argument rules (a key may be abbreviated to a unique prefix; repeatable switch options keep their order), defaults and warnings. A UPF file is Tcl: a variable or command substitution, or any other Tcl command, is refused (exit 3), not guessed at. An error the reference stops on stops the read with exit 2 and writes nothing.",
    "A level shifter's cell (`set_level_shifter_cell`) is held in memory only, as in the reference: it is not part of the stored database, so it is gone from the database `run` writes."
  ],
  "invocation": { "args_template": ["write", "{design}", "-o", "{out}"], "emits_json": false },
  "inputs": { "type": "object", "properties": { "design": { "type": "string", "description": "design database (.odb)" }, "out": { "type": "string", "description": "UPF file to write" } }, "required": ["design", "out"] },
  "artifacts": [ { "role": "upf", "from_arg": "out" } ],
  "assertion": { "id": "upf-written", "not_applicable": true },
  "consumes": ["odb"],
  "threads": {
    "parallel": false,
    "guidance": "Runs on one thread; there is no thread option. Several jobs can run side by side, one core each."
  }
}"#;

/// The reference commit the database binding was built against. The descriptor carries a TOKEN in
/// source and the build's value at run time, so it can never claim a pin the binary does not have.
const CRATE_PIN: &str = vyges_opendb::OPENROAD_PIN;
const PIN_TOKEN: &str = "@OPENROAD_PIN@";

fn describe() -> String {
    DESCRIBE.replace(PIN_TOKEN, CRATE_PIN)
}

fn refuse(what: &str) -> ExitCode {
    vyges_events::emit(
        &vyges_events::Event::new("vyges-upf", vyges_events::Severity::Error, format!("{what} is not implemented in this build"))
            .with_code("UPF-REFUSED"),
    );
    ExitCode::from(3)
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("vyges-upf: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}

/// `write <design.odb> [-o FILE]`: the database's power intent as UPF, to FILE or stdout.
fn write(args: &[String]) -> ExitCode {
    let mut design = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" => match it.next() {
                Some(f) => out = Some(f.clone()),
                None => return usage_error("-o needs a file"),
            },
            "-q" | "--quiet" | "-v" | "--verbose" => {}
            f if f.starts_with('-') => return usage_error(&format!("unknown option `{f}`")),
            f if design.is_none() => design = Some(f.to_string()),
            f => return usage_error(&format!("unexpected argument `{f}`")),
        }
    }
    let Some(design) = design else { return usage_error("write needs a design database") };
    let db = match vyges_opendb::Db::open(&design) {
        Ok(db) => db,
        Err(e) => return input_error(&format!("cannot read {design}: {e}")),
    };
    let pi = match vyges_upf::odb::read_power_intent(&db) {
        Ok(pi) => pi,
        Err(e) => return input_error(&format!("cannot read the power intent of {design}: {e}")),
    };
    let text = vyges_upf::write_upf(&pi);
    match out {
        Some(f) => {
            if let Err(e) = std::fs::write(&f, text) {
                return input_error(&format!("cannot write {f}: {e}"));
            }
        }
        None => print!("{text}"),
    }
    ExitCode::SUCCESS
}

/// `run <design.odb> <intent.upf> -o OUT.odb [--trace]`: the intent read into the database, which
/// is written to OUT only when the whole file was read.
fn run(args: &[String]) -> ExitCode {
    let mut pos = Vec::new();
    let mut out = None;
    let mut trace = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" => match it.next() {
                Some(f) => out = Some(f.clone()),
                None => return usage_error("-o needs a file"),
            },
            "--trace" => trace = true,
            "-q" | "--quiet" | "-v" | "--verbose" => {}
            f if f.starts_with('-') => return usage_error(&format!("unknown option `{f}`")),
            f => pos.push(f.to_string()),
        }
    }
    let ([design, upf], Some(out)) = (<[String; 2]>::try_from(pos).unwrap_or_default(), out) else {
        return usage_error("run needs a design database, a UPF file and -o OUT.odb");
    };
    if design.is_empty() {
        return usage_error("run needs a design database, a UPF file and -o OUT.odb");
    }
    let mut db = match vyges_opendb::Db::open(&design) {
        Ok(db) => db,
        Err(e) => return input_error(&format!("cannot read {design}: {e}")),
    };
    let src = match std::fs::read_to_string(&upf) {
        Ok(s) => s,
        Err(e) => return input_error(&format!("cannot read {upf}: {e}")),
    };
    let mut log = vyges_upf::read::Log { trace, echo: true, ..Default::default() };
    match vyges_upf::read::read_upf(&mut db, &src, &mut log) {
        Ok(n) => {
            if let Err(e) = db.write(&out) {
                return input_error(&format!("cannot write {out}: {e}"));
            }
            eprintln!("vyges-upf: {n} command(s) read from {upf}, {} warning(s); wrote {out}", log.warnings);
            ExitCode::SUCCESS
        }
        Err(stop) => {
            use vyges_upf::tcl::Stop;
            let code = matches!(stop, Stop::Refused(_));
            // The database's logger has printed its own error already.
            if !matches!(stop, Stop::Database { .. }) {
                println!("{stop}");
            }
            vyges_events::emit(
                &vyges_events::Event::new("vyges-upf", vyges_events::Severity::Error, format!("{upf}: {stop}; nothing written"))
                    .with_code(if code { "UPF-REFUSED" } else { "UPF-INPUT" }),
            );
            ExitCode::from(if code { 3 } else { 2 })
        }
    }
}

fn eval(args: &[String]) -> ExitCode {
    let mut pos = Vec::new();
    let mut out = None;
    let mut trace = false;
    let mut liberty = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" => match it.next() {
                Some(f) => out = Some(f.clone()),
                None => return usage_error("-o needs a file"),
            },
            "--liberty" => match it.next() {
                Some(f) => liberty.push(f.clone()),
                None => return usage_error("--liberty needs a file"),
            },
            "--trace" => trace = true,
            "-q" | "--quiet" | "-v" | "--verbose" => {}
            f if f.starts_with('-') => return usage_error(&format!("unknown option `{f}`")),
            f => pos.push(f.to_string()),
        }
    }
    let mut libs = vyges_upf::eval::Liberty::default();
    for f in &liberty {
        let read = std::fs::read_to_string(f)
            .map_err(|e| e.to_string())
            .and_then(|text| vyges_sta::liberty_parse::parse(&text))
            .and_then(|g| vyges_sta::liberty::Library::read(&g));
        match read {
            Ok(lib) => libs.0.push(lib),
            Err(e) => return input_error(&format!("cannot read {f}: {e}")),
        }
    }
    let ([design], Some(out)) = (<[String; 1]>::try_from(pos).unwrap_or_default(), out) else {
        return usage_error("eval needs a design database and -o OUT.odb");
    };
    if design.is_empty() {
        return usage_error("eval needs a design database and -o OUT.odb");
    }
    let mut db = match vyges_opendb::Db::open(&design) {
        Ok(db) => db,
        Err(e) => return input_error(&format!("cannot read {design}: {e}")),
    };
    let mut log = vyges_upf::read::Log { trace, echo: true, ..Default::default() };
    match vyges_upf::eval::eval_upf(&mut db, &libs, &mut log) {
        Ok(done) => {
            if let Err(e) = db.write(&out) {
                return input_error(&format!("cannot write {out}: {e}"));
            }
            // The reference's floorplan ignores a false return and carries on; so does this.
            let how = if done { "evaluated" } else { "evaluation stopped early" };
            eprintln!("vyges-upf: power intent {how}, {} warning(s); wrote {out}", log.warnings);
            ExitCode::SUCCESS
        }
        Err(stop) => {
            use vyges_upf::tcl::Stop;
            let code = matches!(stop, Stop::Refused(_));
            if !matches!(stop, Stop::Database { .. }) {
                println!("{stop}");
            }
            vyges_events::emit(
                &vyges_events::Event::new("vyges-upf", vyges_events::Severity::Error, format!("{design}: {stop}; nothing written"))
                    .with_code(if code { "UPF-REFUSED" } else { "UPF-INPUT" }),
            );
            ExitCode::from(if code { 3 } else { 2 })
        }
    }
}

fn input_error(msg: &str) -> ExitCode {
    vyges_events::emit(&vyges_events::Event::new("vyges-upf", vyges_events::Severity::Error, msg.to_string()).with_code("UPF-INPUT"));
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("-V") | Some("--version") => {
            println!("vyges-upf {} ({})", env!("CARGO_PKG_VERSION"), env!("VYGES_GIT_SHA"));
            println!("Copyright 2026 Vyges. Apache-2.0.");
            ExitCode::SUCCESS
        }
        Some("--describe") => {
            println!("{}", describe());
            ExitCode::SUCCESS
        }
        Some("write") => write(&args[1..]),
        Some("run") => run(&args[1..]),
        Some("eval") => eval(&args[1..]),
        Some(cmd) if COMMANDS.contains(&cmd) => refuse(cmd),
        Some(other) => {
            eprintln!("vyges-upf: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_descriptor_is_valid_json_with_a_legal_maturity() {
        let d: serde_json::Value = serde_json::from_str(&describe()).expect("descriptor is valid JSON");
        assert_eq!(d["schema"], "vyges-tool-descriptor/1.1");
        assert_eq!(d["name"], "upf");
        // Anything else is read as `discovered` and every verdict becomes `unknown`.
        let m = d["maturity"].as_str().unwrap();
        assert!(["discovered", "structured", "workflow-validated"].contains(&m), "{m}");
    }

    /// The schema's predicates and artifact sources, not look-alikes: an `equals` predicate or an
    /// artifact without `field`/`from_arg` is dropped by the registry without a word.
    #[test]
    fn the_descriptor_uses_the_schemas_own_words() {
        let d: serde_json::Value = serde_json::from_str(&describe()).unwrap();
        assert_eq!(d["assertion"]["not_applicable"], true, "write establishes no engineering claim");
        for a in d["artifacts"].as_array().unwrap() {
            assert!(a["role"].is_string() && (a["field"].is_string() || a["from_arg"].is_string()), "{a}");
        }
        assert_eq!(d["threads"]["parallel"], false);
    }

    #[test]
    fn the_descriptor_reports_the_built_pin_and_the_source_keeps_the_token() {
        // `--pins` and the box's pin check read this field; a literal would go stale on a re-pin.
        let d: serde_json::Value = serde_json::from_str(&describe()).unwrap();
        assert_eq!(d["openroad_pin"], CRATE_PIN);
        assert_eq!(CRATE_PIN.len(), 40, "a full commit SHA");
        let raw: serde_json::Value = serde_json::from_str(DESCRIBE).unwrap();
        assert_eq!(raw["openroad_pin"], PIN_TOKEN);
    }

    #[test]
    fn the_usage_spells_the_engine_under_its_group() {
        // The dispatcher REFUSES a wrong group prefix, so the help must spell the right one.
        for c in COMMANDS {
            assert!(USAGE.contains(&format!("vyges physical upf {c}")), "{c}");
        }
    }
}
