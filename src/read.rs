// SPDX-License-Identifier: Apache-2.0
//! `run`: read a UPF file into the design database, as the reference's `read_upf` does.
//!
//! Three layers, each the reference's own, in its order:
//! 1. **the command procs** ([`create_power_domain`], [`create_power_switch`], ...): argument
//!    parsing, defaults and the per-element loops, one function per UPF command;
//! 2. **the `*_cmd` calls** ([`Cmd`]): one per call the procs make into the database layer. Each
//!    prints a `VYG-UPF` trace line when tracing, in the same form as the reference's call-trace
//!    probe, so the two sequences can be compared line for line;
//! 3. **the database functions** (`create_power_domain_db`, `set_isolation_db`, ...): the
//!    lookups, warnings and database calls, through [`UpfStore`].
//!
//! Warnings are logged as the reference logs them (`[WARNING UPF-0001] ...`) and reading goes on;
//! an error stops it ([`Stop`]), as an error stops the reference's `source`.

use crate::tcl::{self, check_argc_eq0, check_argc_eq1, parse_key_args, parse_list_args, split_list, Stop};

/// The database calls the reader makes, one per call the reference makes. Each `*_create` returns
/// `false` where the database refuses (the name is taken; a level shifter's domain is missing).
#[allow(clippy::too_many_arguments)]
pub trait UpfStore {
    /// `getChip() != NULL`, and `getChip()->getBlock() != NULL`.
    fn has_chip(&self) -> bool;
    fn has_block(&self) -> bool;
    /// Does the object exist? `kind`: power_domain, logic_port, power_switch, isolation,
    /// level_shifter, master.
    fn find(&self, kind: &str, name: &str) -> Result<bool, String>;
    fn dbu_per_micron(&self) -> Result<i32, String>;

    fn pwr_domain_create(&mut self, name: &str) -> Result<bool, String>;
    fn pwr_domain_add_element(&mut self, pd: &str, element: &str) -> Result<(), String>;
    fn pwr_domain_add_power_switch(&mut self, pd: &str, sw: &str) -> Result<(), String>;
    fn pwr_domain_add_isolation(&mut self, pd: &str, iso: &str) -> Result<(), String>;
    fn pwr_domain_set_area(&mut self, pd: &str, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<(), String>;
    fn pwr_domain_set_voltage(&mut self, pd: &str, v: f32) -> Result<(), String>;

    fn logic_port_create(&mut self, name: &str, direction: &str) -> Result<bool, String>;

    fn pwr_switch_create(&mut self, name: &str) -> Result<bool, String>;
    fn pwr_switch_set_power_domain(&mut self, sw: &str, pd: &str) -> Result<(), String>;
    fn pwr_switch_set_out_supply_port(&mut self, sw: &str, port: &str, net: &str) -> Result<(), String>;
    fn pwr_switch_add_in_supply_port(&mut self, sw: &str, port: &str, net: &str) -> Result<(), String>;
    fn pwr_switch_add_control_port(&mut self, sw: &str, port: &str, net: &str) -> Result<(), String>;
    fn pwr_switch_add_acknowledge_port(&mut self, sw: &str, port: &str, net: &str, expr: &str) -> Result<(), String>;
    fn pwr_switch_add_on_state(&mut self, sw: &str, state: &str, port: &str, expr: &str) -> Result<(), String>;
    fn pwr_switch_set_lib_cell(&mut self, sw: &str, master: &str) -> Result<(), String>;
    /// `addPortMap(model port, switch port)`; `Err` is the database's own error (it stops the read).
    fn pwr_switch_add_port_map(&mut self, sw: &str, model_port: &str, switch_port: &str) -> Result<(), String>;

    fn isolation_create(&mut self, name: &str) -> Result<bool, String>;
    fn isolation_set_power_domain(&mut self, iso: &str, pd: &str) -> Result<(), String>;
    /// `field`: applies_to, clamp_value, isolation_signal, isolation_sense, location.
    fn isolation_set(&mut self, iso: &str, field: &str, value: &str) -> Result<(), String>;
    fn isolation_add_isolation_cell(&mut self, iso: &str, cell: &str) -> Result<(), String>;

    fn level_shifter_create(&mut self, name: &str, pd: &str) -> Result<bool, String>;
    /// `field`: source, sink, applies_to, applies_to_boundary, rule, location, input_supply,
    /// output_supply, internal_supply, name_prefix, name_suffix, cell_name, cell_input, cell_output.
    fn level_shifter_set(&mut self, ls: &str, field: &str, value: &str) -> Result<(), String>;
    fn level_shifter_set_threshold(&mut self, ls: &str, v: f32) -> Result<(), String>;
    /// `field`: no_shift, force_shift, use_functional_equivalence.
    fn level_shifter_set_bool(&mut self, ls: &str, field: &str, v: bool) -> Result<(), String>;
    fn level_shifter_add_element(&mut self, ls: &str, element: &str) -> Result<(), String>;
    fn level_shifter_add_exclude_element(&mut self, ls: &str, element: &str) -> Result<(), String>;
    fn level_shifter_add_instance(&mut self, ls: &str, inst: &str, port: &str) -> Result<(), String>;
}

/// What a read logs, in order: the reference's warnings, and (when tracing) one `VYG-UPF` line per
/// database-layer call.
#[derive(Debug, Default)]
pub struct Log {
    pub trace: bool,
    /// Print each line as it is logged, so it interleaves with what the database's own logger
    /// prints in the order things happened; the lines are kept either way.
    pub echo: bool,
    pub lines: Vec<String>,
    pub warnings: usize,
}

impl Log {
    pub(crate) fn push(&mut self, l: String) {
        if self.echo {
            println!("{l}");
        }
        self.lines.push(l);
    }
    pub(crate) fn warn(&mut self, tool: &str, id: u32, msg: String) {
        self.warnings += 1;
        self.push(format!("[WARNING {tool}-{id:04}] {msg}"));
    }
}

/// A database call failing stops the read: as the database's own error when it carries a message
/// id (`ODB-0154: ...`, already printed by its logger), otherwise as a refusal naming the database.
pub(crate) fn db<T>(r: Result<T, String>) -> Result<T, Stop> {
    r.map_err(|e| {
        let coded = e.split_once(": ").and_then(|(code, msg)| {
            let (tool, id) = code.split_once('-')?;
            let ok = !tool.is_empty() && tool.chars().all(|c| c.is_ascii_uppercase()) && id.len() == 4;
            Some(Stop::Database { tool: tool.into(), id: id.parse().ok().filter(|_| ok)?, msg: msg.into() })
        });
        coded.unwrap_or_else(|| Stop::Refused(format!("database: {e}")))
    })
}

/// The UPF commands this reader evaluates, in the order the reference defines them.
pub const UPF_COMMANDS: &[&str] = &[
    "create_power_domain",
    "create_logic_port",
    "create_power_switch",
    "set_isolation",
    "use_interface_cell",
    "set_domain_area",
    "map_power_switch",
    "set_level_shifter",
    "set_domain_voltage",
    "set_level_shifter_cell",
];

/// `read_upf -file F`: every command of the file, in order. Stops at the first error, as `source`
/// does; what was applied before it stays applied.
pub fn read_upf(store: &mut dyn UpfStore, src: &str, log: &mut Log) -> Result<usize, Stop> {
    let cmds = tcl::parse_script(src)?;
    let mut n = 0;
    for c in cmds {
        let (name, args) = (c.words[0].as_str(), c.words[1..].to_vec());
        let mut x = Cmd { store, log };
        match name {
            "create_power_domain" => create_power_domain(&mut x, args),
            "create_logic_port" => create_logic_port(&mut x, args),
            "create_power_switch" => create_power_switch(&mut x, args),
            "set_isolation" => set_isolation(&mut x, args),
            "use_interface_cell" => use_interface_cell(&mut x, args),
            "set_domain_area" => set_domain_area(&mut x, args),
            "map_power_switch" => map_power_switch(&mut x, args),
            "set_level_shifter" => set_level_shifter(&mut x, args),
            "set_domain_voltage" => set_domain_voltage(&mut x, args),
            "set_level_shifter_cell" => set_level_shifter_cell(&mut x, args),
            other => Err(Stop::Refused(format!("line {}: `{other}` is not a UPF command this reader evaluates", c.line))),
        }?;
        n += 1;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------------------------
// 1. The command procs, in the reference's order.
// ---------------------------------------------------------------------------------------------

/// `upf::check_block_exists`.
fn check_block_exists(x: &Cmd) -> Result<(), Stop> {
    if !x.store.has_chip() {
        return Err(Stop::error("UPF", 33, "No Chip exists"));
    }
    if !x.store.has_block() {
        return Err(Stop::error("UPF", 34, "No block exists"));
    }
    Ok(())
}

/// `upf::process_list_arg`: a list padded with empty elements to `len`.
fn process_list_arg(v: &str, len: usize) -> Result<Vec<String>, Stop> {
    let mut l = split_list(v)?;
    while l.len() < len {
        l.push(String::new());
    }
    Ok(l)
}

fn key(k: &tcl::KeyArgs, name: &str) -> String {
    k.keys.get(name).cloned().unwrap_or_default()
}

fn create_power_domain(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args("create_power_domain", args, &["-elements"], &[])?;
    check_argc_eq1("create_power_domain", &k.args)?;
    let domain_name = k.args[0].clone();
    let elements = split_list(&key(&k, "-elements"))?;
    x.create_power_domain_cmd(&domain_name)?;
    for el in &elements {
        x.update_power_domain_cmd(&domain_name, el)?;
    }
    Ok(())
}

fn create_logic_port(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args("create_logic_port", args, &["-direction"], &[])?;
    check_argc_eq1("create_logic_port", &k.args)?;
    let port_name = k.args[0].clone();
    x.create_logic_port_cmd(&port_name, &key(&k, "-direction"))
}

fn create_power_switch(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let (args, list) =
        parse_list_args("create_power_switch", args, &["-input_supply_port", "-control_port", "-ack_port", "-on_state"])?;
    let k = parse_key_args("create_power_switch", args, &["-domain", "-output_supply_port"], &[])?;
    check_argc_eq1("create_power_switch", &k.args)?;
    let name = k.args[0].clone();
    x.create_power_switch_cmd(&name, &key(&k, "-domain"))?;
    if let Some(v) = k.keys.get("-output_supply_port") {
        let l = process_list_arg(v, 2)?;
        x.update_power_switch_output_cmd(&name, &l[0], &l[1])?;
    }
    for v in &list["-input_supply_port"] {
        let l = process_list_arg(v, 2)?;
        x.update_power_switch_input_cmd(&name, &l[0], &l[1])?;
    }
    for v in &list["-control_port"] {
        let l = process_list_arg(v, 2)?;
        x.update_power_switch_control_cmd(&name, &l[0], &l[1])?;
    }
    for v in &list["-ack_port"] {
        let l = process_list_arg(v, 3)?;
        x.update_power_switch_ack_cmd(&name, &l[0], &l[1], &l[2])?;
    }
    for v in &list["-on_state"] {
        let l = process_list_arg(v, 3)?;
        x.update_power_switch_on_cmd(&name, &l[0], &l[1], &l[2])?;
    }
    Ok(())
}

fn set_isolation(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args(
        "set_isolation",
        args,
        &["-domain", "-applies_to", "-clamp_value", "-isolation_signal", "-isolation_sense", "-location"],
        &["-update"],
    )?;
    check_argc_eq1("set_isolation", &k.args)?;
    let name = k.args[0].clone();
    let Some(domain) = k.keys.get("-domain") else {
        return Err(Stop::error("UPF", 73, "-domain is required for set_isolation"));
    };
    // Without -update a new strategy defaults to sense `high` and location `self`.
    let update = k.flags.contains("-update");
    let (mut sense, mut location) = if update { (String::new(), String::new()) } else { ("high".into(), "self".into()) };
    if let Some(v) = k.keys.get("-isolation_sense") {
        sense = v.clone();
    }
    if let Some(v) = k.keys.get("-location") {
        location = v.clone();
    }
    x.set_isolation_cmd(&name, domain, update, &key(&k, "-applies_to"), &key(&k, "-clamp_value"), &key(&k, "-isolation_signal"), &sense, &location)
}

fn use_interface_cell(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args("use_interface_cell", args, &["-domain", "-strategy", "-lib_cells"], &[])?;
    check_argc_eq1("use_interface_cell", &k.args)?;
    let Some(domain) = k.keys.get("-domain") else {
        return Err(Stop::error("UPF", 75, "-domain is required for use_interface_cell"));
    };
    let Some(strategy) = k.keys.get("-strategy") else {
        return Err(Stop::error("UPF", 76, "-strategy is required for use_interface_cell"));
    };
    let Some(lib_cells) = k.keys.get("-lib_cells") else {
        return Err(Stop::error("UPF", 77, "-lib_cells is required for use_interface_cell"));
    };
    for strat in split_list(strategy)? {
        for cell in split_list(lib_cells)? {
            x.use_interface_cell_cmd(domain, &strat, &cell)?;
        }
    }
    Ok(())
}

fn set_domain_area(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args("set_domain_area", args, &["-area"], &[])?;
    let Some(area) = k.keys.get("-area") else {
        return Err(Stop::error("UPF", 37, "please define area"));
    };
    let area = split_list(area)?;
    if area.len() != 4 {
        return Err(Stop::error("UPF", 36, "-area is a list of 4 coordinates"));
    }
    let mut um = [0f64; 4];
    for (v, s) in um.iter_mut().zip(&area) {
        // `sta::check_positive_float`: a number, and not negative.
        match s.trim().parse::<f64>() {
            Ok(f) if tcl::is_double(s) && f >= 0.0 => *v = f,
            _ => return Err(Stop::error("STA", 572, format!("-area '{s}' is not a positive float."))),
        }
    }
    check_argc_eq1("set_domain_area", &k.args)?;
    // `set domain_name $args`: the one-element list, as a string.
    let domain_name = k.args[0].clone();
    let dbu = db(x.store.dbu_per_micron())? as f64;
    // `ord::microns_to_dbu`: std::round(microns * dbu per micron).
    let [lx, ly, ux, uy] = um.map(|m| (m * dbu).round() as i32);
    x.set_domain_area_cmd(&domain_name, lx, ly, ux, uy)
}

fn map_power_switch(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args("map_power_switch", args, &["-switch_name_list", "-lib_cells", "-port_map"], &[])?;
    check_argc_eq1("map_power_switch", &k.args)?;
    // ⚠️ The switches come from the POSITIONAL argument; `-switch_name_list` is parsed and unused.
    let switches = split_list(&k.args[0])?;
    let lib_cells = split_list(&key(&k, "-lib_cells"))?;
    let port_map = split_list(&key(&k, "-port_map"))?;
    for (i, switch) in switches.iter().enumerate() {
        // `lindex` past the end is the empty string.
        let cell = lib_cells.get(i).cloned().unwrap_or_default();
        x.set_power_switch_cell(switch, &cell)?;
        for port in &port_map {
            let p = split_list(port)?;
            if p.len() != 2 {
                return Err(Stop::error("UPF", 40, "The port map should be a list of exactly 2 elements"));
            }
            x.set_power_switch_port_map(switch, &p[0], &p[1])?;
        }
    }
    Ok(())
}

fn set_level_shifter(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args(
        "set_level_shifter",
        args,
        &[
            "-domain",
            "-elements",
            "-exclude_elements",
            "-source",
            "-sink",
            "-use_functional_equivalence",
            "-applies_to",
            "-applies_to_boundary",
            "-rule",
            "-threshold",
            "-location",
            "-input_supply",
            "-output_supply",
            "-internal_supply",
            "-name_prefix",
            "-name_suffix",
            "-instance",
            "-use_equivalence",
        ],
        &["-update", "-no_shift", "-force_shift"],
    )?;
    check_argc_eq1("set_level_shifter", &k.args)?;
    let name = k.args[0].clone();
    let ufe = k.keys.get("-use_functional_equivalence").cloned().unwrap_or_else(|| "TRUE".into());
    if k.keys.contains_key("-use_equivalence") {
        x.log.warn("UPF", 57, "-use_equivalence is deprecated in UPF and not supported".into());
    }
    let flag = |f: &str| if k.flags.contains(f) { "1".to_string() } else { String::new() };
    let a = |n: &str| key(&k, n);
    x.create_or_update_level_shifter_cmd(&LevelShifterArgs {
        name: name.clone(),
        domain: a("-domain"),
        source: a("-source"),
        sink: a("-sink"),
        use_functional_equivalence: ufe,
        applies_to: a("-applies_to"),
        applies_to_boundary: a("-applies_to_boundary"),
        rule: a("-rule"),
        threshold: a("-threshold"),
        no_shift: flag("-no_shift"),
        force_shift: flag("-force_shift"),
        location: a("-location"),
        input_supply: a("-input_supply"),
        output_supply: a("-output_supply"),
        internal_supply: a("-internal_supply"),
        name_prefix: a("-name_prefix"),
        name_suffix: a("-name_suffix"),
        update: k.flags.contains("-update"),
    })?;
    // ⚠️ The proc's `if { $ok == 0 } { return }` never fires: the call it tests returns nothing,
    // and "" is not 0. So the element, exclusion and instance calls run even when the strategy was
    // not created, each warning on its own.
    for element in split_list(&a("-elements"))? {
        x.add_level_shifter_element_cmd(&name, &element)?;
    }
    for element in split_list(&a("-exclude_elements"))? {
        x.exclude_level_shifter_element_cmd(&name, &element)?;
    }
    for inst_port in split_list(&a("-instance"))? {
        let p = split_list(&inst_port)?;
        let get = |i: usize| p.get(i).cloned().unwrap_or_default();
        x.handle_level_shifter_instance_cmd(&name, &get(0), &get(1))?;
    }
    Ok(())
}

fn set_domain_voltage(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args("set_domain_voltage", args, &["-domain", "-voltage"], &[])?;
    check_argc_eq0("set_domain_voltage", &k.args)?;
    let voltage = k.keys.get("-voltage").cloned().unwrap_or_else(|| "0.0".into());
    x.set_domain_voltage_cmd(&key(&k, "-domain"), &voltage)
}

fn set_level_shifter_cell(x: &mut Cmd, args: Vec<String>) -> Result<(), Stop> {
    check_block_exists(x)?;
    let k = parse_key_args("set_level_shifter_cell", args, &["-level_shifter", "-cell_name", "-input_port", "-output_port"], &[])?;
    check_argc_eq0("set_level_shifter_cell", &k.args)?;
    x.set_level_shifter_cell_cmd(&key(&k, "-level_shifter"), &key(&k, "-cell_name"), &key(&k, "-input_port"), &key(&k, "-output_port"))
}

// ---------------------------------------------------------------------------------------------
// 2. The `*_cmd` calls: trace, then the database function.
// ---------------------------------------------------------------------------------------------

/// The arguments of `create_or_update_level_shifter_cmd`, as the proc passes them (strings).
#[derive(Debug, Default, Clone)]
pub struct LevelShifterArgs {
    pub name: String,
    pub domain: String,
    pub source: String,
    pub sink: String,
    pub use_functional_equivalence: String,
    pub applies_to: String,
    pub applies_to_boundary: String,
    pub rule: String,
    pub threshold: String,
    pub no_shift: String,
    pub force_shift: String,
    pub location: String,
    pub input_supply: String,
    pub output_supply: String,
    pub internal_supply: String,
    pub name_prefix: String,
    pub name_suffix: String,
    pub update: bool,
}

struct Cmd<'a> {
    store: &'a mut dyn UpfStore,
    log: &'a mut Log,
}

impl Cmd<'_> {
    fn trace(&mut self, cmd: &str, args: &[&str]) {
        if self.log.trace {
            let mut l = format!("VYG-UPF {cmd}");
            for a in args {
                l += &format!(" <{a}>");
            }
            self.log.push(l);
        }
    }

    fn create_power_domain_cmd(&mut self, name: &str) -> Result<(), Stop> {
        self.trace("create_power_domain_cmd", &[name]);
        create_power_domain_db(self, name)
    }
    fn update_power_domain_cmd(&mut self, name: &str, element: &str) -> Result<(), Stop> {
        self.trace("update_power_domain_cmd", &[name, element]);
        update_power_domain_db(self, name, element)
    }
    fn create_logic_port_cmd(&mut self, name: &str, direction: &str) -> Result<(), Stop> {
        self.trace("create_logic_port_cmd", &[name, direction]);
        create_logic_port_db(self, name, direction)
    }
    fn create_power_switch_cmd(&mut self, name: &str, domain: &str) -> Result<(), Stop> {
        self.trace("create_power_switch_cmd", &[name, domain]);
        create_power_switch_db(self, name, domain)
    }
    fn update_power_switch_control_cmd(&mut self, name: &str, port: &str, net: &str) -> Result<(), Stop> {
        self.trace("update_power_switch_control_cmd", &[name, port, net]);
        update_power_switch_db(self, name, 6, &format!("adding control port {port}"), |s, sw| s.pwr_switch_add_control_port(sw, port, net))
    }
    fn update_power_switch_ack_cmd(&mut self, name: &str, port: &str, net: &str, b: &str) -> Result<(), Stop> {
        self.trace("update_power_switch_ack_cmd", &[name, port, net, b]);
        update_power_switch_db(self, name, 61, &format!("adding acknowledge port {port}"), |s, sw| {
            s.pwr_switch_add_acknowledge_port(sw, port, net, b)
        })
    }
    fn update_power_switch_on_cmd(&mut self, name: &str, state: &str, port: &str, b: &str) -> Result<(), Stop> {
        self.trace("update_power_switch_on_cmd", &[name, state, port, b]);
        update_power_switch_db(self, name, 7, &format!("adding on state {state}"), |s, sw| s.pwr_switch_add_on_state(sw, state, port, b))
    }
    fn update_power_switch_input_cmd(&mut self, name: &str, port: &str, net: &str) -> Result<(), Stop> {
        self.trace("update_power_switch_input_cmd", &[name, port, net]);
        update_power_switch_db(self, name, 8, &format!("adding input port {port}"), |s, sw| s.pwr_switch_add_in_supply_port(sw, port, net))
    }
    fn update_power_switch_output_cmd(&mut self, name: &str, port: &str, net: &str) -> Result<(), Stop> {
        self.trace("update_power_switch_output_cmd", &[name, port, net]);
        update_power_switch_db(self, name, 9, &format!("adding output port {port}"), |s, sw| s.pwr_switch_set_out_supply_port(sw, port, net))
    }
    #[allow(clippy::too_many_arguments)]
    fn set_isolation_cmd(
        &mut self,
        name: &str,
        domain: &str,
        update: bool,
        applies_to: &str,
        clamp_value: &str,
        signal: &str,
        sense: &str,
        location: &str,
    ) -> Result<(), Stop> {
        let u = if update { "1" } else { "0" };
        self.trace("set_isolation_cmd", &[name, domain, u, applies_to, clamp_value, signal, sense, location]);
        set_isolation_db(self, name, domain, update, &[("applies_to", applies_to), ("clamp_value", clamp_value), ("isolation_signal", signal), ("isolation_sense", sense), ("location", location)])
    }
    fn use_interface_cell_cmd(&mut self, domain: &str, strategy: &str, cell: &str) -> Result<(), Stop> {
        self.trace("use_interface_cell_cmd", &[domain, strategy, cell]);
        use_interface_cell_db(self, domain, strategy, cell)
    }
    fn set_domain_area_cmd(&mut self, domain: &str, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<(), Stop> {
        // The probe prints the Rect's corners (the reference passes an object, not text).
        self.trace("set_domain_area_cmd", &[domain, &format!("{x1} {y1} {x2} {y2}")]);
        set_domain_area_db(self, domain, x1, y1, x2, y2)
    }
    /// The SWIG function itself: the cell is looked up in every library, in the database's order;
    /// none is an error.
    fn set_power_switch_cell(&mut self, name: &str, cell: &str) -> Result<(), Stop> {
        self.trace("set_power_switch_cell", &[name, cell]);
        if !db(self.store.find("master", cell))? {
            return Err(Stop::error("UPF", 43, format!("Cannot find master {cell}")));
        }
        update_power_switch_db(self, name, 10, &format!("adding cell {cell}"), |s, sw| s.pwr_switch_set_lib_cell(sw, cell))
    }
    fn set_power_switch_port_map(&mut self, name: &str, model_port: &str, switch_port: &str) -> Result<(), Stop> {
        self.trace("set_power_switch_port_map", &[name, model_port, switch_port]);
        update_power_switch_db(self, name, 11, "updating port mapping", |s, sw| s.pwr_switch_add_port_map(sw, model_port, switch_port))
    }
    fn create_or_update_level_shifter_cmd(&mut self, a: &LevelShifterArgs) -> Result<(), Stop> {
        let u = if a.update { "1" } else { "0" };
        self.trace(
            "create_or_update_level_shifter_cmd",
            &[
                &a.name,
                &a.domain,
                &a.source,
                &a.sink,
                &a.use_functional_equivalence,
                &a.applies_to,
                &a.applies_to_boundary,
                &a.rule,
                &a.threshold,
                &a.no_shift,
                &a.force_shift,
                &a.location,
                &a.input_supply,
                &a.output_supply,
                &a.internal_supply,
                &a.name_prefix,
                &a.name_suffix,
                u,
            ],
        );
        create_or_update_level_shifter_db(self, a)
    }
    fn add_level_shifter_element_cmd(&mut self, ls: &str, element: &str) -> Result<(), Stop> {
        self.trace("add_level_shifter_element_cmd", &[ls, element]);
        level_shifter_db(self, ls, 39, &format!("to add element {element}"), |s| s.level_shifter_add_element(ls, element))
    }
    fn exclude_level_shifter_element_cmd(&mut self, ls: &str, element: &str) -> Result<(), Stop> {
        self.trace("exclude_level_shifter_element_cmd", &[ls, element]);
        level_shifter_db(self, ls, 41, &format!("to exclude element {element}"), |s| s.level_shifter_add_exclude_element(ls, element))
    }
    fn handle_level_shifter_instance_cmd(&mut self, ls: &str, inst: &str, port: &str) -> Result<(), Stop> {
        self.trace("handle_level_shifter_instance_cmd", &[ls, inst, port]);
        level_shifter_db(self, ls, 42, &format!("to add instance {inst}"), |s| s.level_shifter_add_instance(ls, inst, port))
    }
    /// The voltage reaches the database as a float; text that is not a number stops the read.
    fn set_domain_voltage_cmd(&mut self, domain: &str, voltage: &str) -> Result<(), Stop> {
        self.trace("set_domain_voltage_cmd", &[domain, voltage]);
        let Some(v) = tcl::is_double(voltage).then(|| voltage.trim().parse::<f64>().ok()).flatten() else {
            return Err(Stop::Refused(format!("set_domain_voltage: -voltage {voltage:?} is not a number")));
        };
        set_domain_voltage_db(self, domain, v as f32)
    }
    fn set_level_shifter_cell_cmd(&mut self, ls: &str, cell: &str, input: &str, output: &str) -> Result<(), Stop> {
        self.trace("set_level_shifter_cell_cmd", &[ls, cell, input, output]);
        level_shifter_db(self, ls, 60, &format!("to set cell {cell}"), |s| {
            s.level_shifter_set(ls, "cell_name", cell)?;
            s.level_shifter_set(ls, "cell_input", input)?;
            s.level_shifter_set(ls, "cell_output", output)
        })
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The database functions: the reference's lookups, warnings and calls.
// ---------------------------------------------------------------------------------------------

fn create_power_domain_db(x: &mut Cmd, name: &str) -> Result<(), Stop> {
    if !db(x.store.pwr_domain_create(name))? {
        x.log.warn("UPF", 1, format!("Creation of {name} power domain failed"));
    }
    Ok(())
}

fn update_power_domain_db(x: &mut Cmd, name: &str, element: &str) -> Result<(), Stop> {
    if db(x.store.find("power_domain", name))? {
        db(x.store.pwr_domain_add_element(name, element))
    } else {
        x.log.warn("UPF", 2, format!("Couldn't retrieve power domain {name} while adding element {element}"));
        Ok(())
    }
}

fn create_logic_port_db(x: &mut Cmd, name: &str, direction: &str) -> Result<(), Stop> {
    if !db(x.store.logic_port_create(name, direction))? {
        x.log.warn("UPF", 3, format!("Creation of {name} logic port failed"));
    }
    Ok(())
}

fn create_power_switch_db(x: &mut Cmd, name: &str, domain: &str) -> Result<(), Stop> {
    if !db(x.store.find("power_domain", domain))? {
        x.log.warn("UPF", 4, format!("Couldn't retrieve power domain {domain} while creating power switch {name}"));
        return Ok(());
    }
    if !db(x.store.pwr_switch_create(name))? {
        x.log.warn("UPF", 5, format!("Creation of {name} power switch failed"));
        return Ok(());
    }
    db(x.store.pwr_switch_set_power_domain(name, domain))?;
    db(x.store.pwr_domain_add_power_switch(domain, name))
}

/// Every `update_power_switch_*`: find the switch (else warning `id`, "Couldn't retrieve power
/// switch NAME while WHAT"), then one call.
fn update_power_switch_db(
    x: &mut Cmd,
    name: &str,
    id: u32,
    what: &str,
    call: impl FnOnce(&mut dyn UpfStore, &str) -> Result<(), String>,
) -> Result<(), Stop> {
    if !db(x.store.find("power_switch", name))? {
        x.log.warn("UPF", id, format!("Couldn't retrieve power switch {name} while {what}"));
        return Ok(());
    }
    db(call(x.store, name))
}

fn set_isolation_db(x: &mut Cmd, name: &str, domain: &str, update: bool, fields: &[(&str, &str)]) -> Result<(), Stop> {
    if !db(x.store.find("power_domain", domain))? {
        x.log.warn("UPF", 12, format!("Couldn't retrieve power domain {domain} while creating/updating isolation {name}"));
        return Ok(());
    }
    let exists = db(x.store.find("isolation", name))?;
    if !exists && update {
        x.log.warn("UPF", 13, format!("Couldn't update a non existing isolation {name}"));
        return Ok(());
    }
    if !exists {
        db(x.store.isolation_create(name))?;
    }
    if !update {
        db(x.store.isolation_set_power_domain(name, domain))?;
        db(x.store.pwr_domain_add_isolation(domain, name))?;
    }
    for (field, value) in fields {
        if !value.is_empty() {
            db(x.store.isolation_set(name, field, value))?;
        }
    }
    Ok(())
}

fn use_interface_cell_db(x: &mut Cmd, domain: &str, strategy: &str, cell: &str) -> Result<(), Stop> {
    if !db(x.store.find("power_domain", domain))? {
        x.log.warn("UPF", 14, format!("Couldn't retrieve power domain {domain} while updating isolation {strategy}"));
        return Ok(());
    }
    if !db(x.store.find("isolation", strategy))? {
        x.log.warn("UPF", 15, format!("Couldn't find isolation {strategy}"));
        return Ok(());
    }
    db(x.store.isolation_add_isolation_cell(strategy, cell))
}

fn set_domain_area_db(x: &mut Cmd, domain: &str, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<(), Stop> {
    if !db(x.store.find("power_domain", domain))? {
        x.log.warn("UPF", 16, format!("Couldn't retrieve power domain {domain} while updating its area "));
        return Ok(());
    }
    db(x.store.pwr_domain_set_area(domain, x1, y1, x2, y2))
}

/// `create_or_update_level_shifter`: find (when updating) or create, then each field the proc
/// passed non-empty, in the reference's order; `use_functional_equivalence` is set every time.
fn create_or_update_level_shifter_db(x: &mut Cmd, a: &LevelShifterArgs) -> Result<(), Stop> {
    let name = a.name.as_str();
    let pd_exists = db(x.store.find("power_domain", &a.domain))?;
    let mut found = false;
    if a.update {
        found = db(x.store.find("level_shifter", name))?;
        if !found {
            x.log.warn("UPF", 32, format!("Couldn't find level shifter {name} to update"));
            return Ok(());
        }
    }
    if !found {
        if !pd_exists {
            x.log.warn("UPF", 38, format!("Couldn't find power domain {} for level shifter {name}", a.domain));
            return Ok(());
        }
        if !db(x.store.level_shifter_create(name, &a.domain))? {
            x.log.warn("UPF", 44, format!("Couldn't create level shifter {name}"));
            return Ok(());
        }
    }
    for (field, value) in [
        ("source", &a.source),
        ("sink", &a.sink),
        ("applies_to", &a.applies_to),
        ("applies_to_boundary", &a.applies_to_boundary),
        ("rule", &a.rule),
        ("location", &a.location),
        ("input_supply", &a.input_supply),
        ("output_supply", &a.output_supply),
        ("internal_supply", &a.internal_supply),
        ("name_prefix", &a.name_prefix),
        ("name_suffix", &a.name_suffix),
    ] {
        if !value.is_empty() {
            db(x.store.level_shifter_set(name, field, value))?;
        }
    }
    if !a.threshold.is_empty() {
        let t = stof(&a.threshold).ok_or_else(|| Stop::Refused(format!("set_level_shifter: -threshold {:?} is not a number", a.threshold)))?;
        db(x.store.level_shifter_set_threshold(name, t))?;
    }
    if !a.no_shift.is_empty() {
        db(x.store.level_shifter_set_bool(name, "no_shift", stoi(&a.no_shift) != 0))?;
    }
    if !a.force_shift.is_empty() {
        db(x.store.level_shifter_set_bool(name, "force_shift", stoi(&a.force_shift) != 0))?;
    }
    db(x.store.level_shifter_set_bool(name, "use_functional_equivalence", a.use_functional_equivalence.to_uppercase() == "TRUE"))
}

/// Every level-shifter adder: find it (else warning `id`, "Couldn't find level shifter NAME WHAT"),
/// then the calls.
fn level_shifter_db(
    x: &mut Cmd,
    ls: &str,
    id: u32,
    what: &str,
    call: impl FnOnce(&mut dyn UpfStore) -> Result<(), String>,
) -> Result<(), Stop> {
    if !db(x.store.find("level_shifter", ls))? {
        x.log.warn("UPF", id, format!("Couldn't find level shifter {ls} {what}"));
        return Ok(());
    }
    db(call(x.store))
}

fn set_domain_voltage_db(x: &mut Cmd, domain: &str, voltage: f32) -> Result<(), Stop> {
    if !db(x.store.find("power_domain", domain))? {
        x.log.warn("UPF", 59, format!("Couldn't find power domain {domain} to set voltage {voltage}"));
        return Ok(());
    }
    db(x.store.pwr_domain_set_voltage(domain, voltage))
}

/// `std::stof`: the longest leading decimal number (after leading whitespace); none is an error.
fn stof(s: &str) -> Option<f32> {
    let t = s.trim_start();
    (1..=t.len()).rev().filter(|&n| t.is_char_boundary(n)).find_map(|n| {
        let p = &t[..n];
        (!p.ends_with(['e', 'E', '+', '-'])).then(|| p.parse::<f32>().ok()).flatten()
    })
}

/// `std::stoi` on the proc's "1" (the only value it passes).
fn stoi(s: &str) -> i64 {
    s.trim().parse().unwrap_or(0)
}

#[cfg(test)]
mod tests;
