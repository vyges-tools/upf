// SPDX-License-Identifier: Apache-2.0
//! `eval`: the power intent in the database made physical — a fence region and a group per power
//! domain, the domain hierarchy, each instance in its domain's group, then isolation cells and
//! level shifters on the connections that cross domains. The floorplan runs this before it builds
//! rows.
//!
//! One function per step, named after the step and kept in the order the reference's source
//! defines them; [`eval_upf`] (last) is nothing but the call sequence. Every database write goes
//! through [`EvalStore`] and, with `--trace`, prints one `VYGEV|…` line in the order made.
//!
//! 🔑 The insertion step walks the LIVE netlist: each terminal's foreign connections are read when
//! it is reached, after the cells already inserted have rewired the nets — so a cell inserted for
//! one domain can be isolated again from the next.

use std::collections::{BTreeMap, HashMap};

use vyges_sta::func_expr::FuncExpr;
use vyges_sta::liberty::{Cell, Library};

use crate::read::Log;
use crate::tcl::Stop;

/// An isolation strategy as the database holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IsolationRule {
    pub applies_to: String,
    pub clamp_value: String,
    pub isolation_signal: String,
    pub isolation_sense: String,
    pub location: String,
    /// `getIsolationCells`: each cell looked up as a master, "" where there is none.
    pub cells: Vec<String>,
}

/// A level-shifter strategy as the database holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShiftRule {
    pub elements: Vec<String>,
    pub exclude_elements: Vec<String>,
    pub force_shift: bool,
    pub no_shift: bool,
    pub rule: String,
    pub threshold: f32,
    pub applies_to: String,
    pub location: String,
    pub cell_name: String,
    pub cell_input: String,
    pub cell_output: String,
}

/// The database reads and writes the evaluation makes, one per call the reference makes. Each
/// `*_create` returns `false` where the database refuses (the name is taken). Lists come in the
/// database's order; an instance terminal is (instance, mterm).
pub trait EvalStore {
    fn logic_ports(&self) -> Result<Vec<String>, String>;
    fn power_domains(&self) -> Result<Vec<String>, String>;
    fn domain_elements(&self, pd: &str) -> Result<Vec<String>, String>;
    /// `getArea`: the area when one was set.
    fn domain_area(&self, pd: &str) -> Result<Option<[i32; 4]>, String>;
    fn domain_is_top(&self, pd: &str) -> Result<bool, String>;
    /// `getParent`, `getGroup`: `None` for a null pointer.
    fn domain_parent(&self, pd: &str) -> Result<Option<String>, String>;
    fn domain_group(&self, pd: &str) -> Result<Option<String>, String>;
    fn domain_voltage(&self, pd: &str) -> Result<f32, String>;
    fn domain_isolations(&self, pd: &str) -> Result<Vec<String>, String>;
    fn domain_level_shifters(&self, pd: &str) -> Result<Vec<String>, String>;
    /// The children of `module` ("" = the top module) in `getChildren` order: (hierarchical name of
    /// the dbModInst, name of its master module).
    fn module_children(&self, module: &str) -> Result<Vec<(String, String)>, String>;
    /// Every instance in `getInsts` order with its module's path: "" for none, "." for the top
    /// module, else the hierarchical name of the module's dbModInst.
    fn inst_module_paths(&self) -> Result<Vec<(String, String)>, String>;
    fn group_insts(&self, group: &str) -> Result<Vec<String>, String>;
    fn inst_master(&self, inst: &str) -> Result<String, String>;
    fn inst_group(&self, inst: &str) -> Result<Option<String>, String>;
    /// An instance's terminals in `getITerms` order: (mterm, io type as odb names it).
    fn inst_iterms(&self, inst: &str) -> Result<Vec<(String, String)>, String>;
    fn iterm_net(&self, inst: &str, mterm: &str) -> Result<Option<String>, String>;
    /// A net's terminals in `getITerms` order: (instance, mterm).
    fn net_iterms(&self, net: &str) -> Result<Vec<(String, String)>, String>;
    /// `getCellName` of every level shifter in the block.
    fn level_shifter_cells(&self) -> Result<Vec<String>, String>;
    /// Does the object exist? `kind`: power_domain, inst, net, master.
    fn exists(&self, kind: &str, name: &str) -> Result<bool, String>;
    fn isolation(&self, iso: &str) -> Result<IsolationRule, String>;
    fn level_shifter(&self, ls: &str) -> Result<ShiftRule, String>;
    /// Every master of every library, in library order.
    fn masters(&self) -> Result<Vec<String>, String>;
    /// A master's mterms in order: (name, io type).
    fn master_mterms(&self, master: &str) -> Result<Vec<(String, String)>, String>;
    /// `dbMaster::getArea`: width * height in DBU².
    fn master_area(&self, master: &str) -> Result<i64, String>;

    fn net_create(&mut self, name: &str) -> Result<bool, String>;
    fn bterm_create(&mut self, net: &str, name: &str) -> Result<bool, String>;
    fn set_top(&mut self, pd: &str, top: bool) -> Result<(), String>;
    fn set_parent(&mut self, pd: &str, parent: &str) -> Result<(), String>;
    fn set_group(&mut self, pd: &str, group: &str) -> Result<(), String>;
    fn region_create(&mut self, name: &str) -> Result<bool, String>;
    /// `setRegionType`: INCLUSIVE, EXCLUSIVE or SUGGESTED.
    fn region_set_type(&mut self, region: &str, kind: &str) -> Result<(), String>;
    /// `dbBox::create(region, …)`.
    fn region_add_boundary(&mut self, region: &str, area: [i32; 4]) -> Result<bool, String>;
    fn group_create(&mut self, region: &str, name: &str) -> Result<bool, String>;
    /// `setType`: PHYSICAL_CLUSTER, VOLTAGE_DOMAIN, POWER_DOMAIN or VISUAL_DEBUG.
    fn group_set_type(&mut self, group: &str, kind: &str) -> Result<(), String>;
    fn group_add_inst(&mut self, group: &str, inst: &str) -> Result<(), String>;
    fn inst_create(&mut self, master: &str, name: &str) -> Result<bool, String>;
    /// `dbITerm::connect` / `disconnect`.
    fn connect(&mut self, inst: &str, mterm: &str, net: &str) -> Result<(), String>;
    fn disconnect(&mut self, inst: &str, mterm: &str) -> Result<(), String>;
}

/// The Liberty libraries, in the order they were read: a master's cell is the first library's cell
/// of that name.
#[derive(Debug, Default)]
pub struct Liberty(pub Vec<Library>);

impl Liberty {
    fn cell(&self, name: &str) -> Option<&Cell> {
        self.0.iter().find_map(|l| l.cells.get(name))
    }
}

/// `path -> power domain`, ordered as the reference's `std::map` iterates it (byte order).
type PathToDomain = BTreeMap<String, String>;

/// An instance terminal.
#[derive(Debug, Clone, PartialEq)]
struct ITerm {
    inst: String,
    mterm: String,
}

impl ITerm {
    /// `dbITerm::getName(sep)`.
    fn name(&self, sep: char) -> String {
        format!("{}{sep}{}", self.inst, self.mterm)
    }
}

/// `find_smallest_inverter`'s answer: the master and its buffer ports.
#[derive(Debug, Clone)]
struct Inverter {
    master: String,
    input: String,
    output: String,
}

/// `find_smallest_isolation`'s answer.
#[derive(Debug, Clone)]
struct IsoCell {
    master: String,
    enable: String,
    data: String,
    output: String,
    invert_control: bool,
    invert_output: bool,
}

fn db<T>(r: Result<T, String>) -> Result<T, Stop> {
    crate::read::db(r)
}

fn trace(log: &mut Log, line: String) {
    if log.trace {
        log.push(line);
    }
}

fn bit(b: bool) -> u8 {
    u8::from(b)
}

fn io_of(s: &impl EvalStore, t: &ITerm) -> Result<String, Stop> {
    Ok(db(s.inst_iterms(&t.inst))?.into_iter().find(|(m, _)| *m == t.mterm).map(|(_, io)| io).unwrap_or_default())
}

fn net_create(s: &mut impl EvalStore, log: &mut Log, name: &str) -> Result<bool, Stop> {
    let ok = db(s.net_create(name))?;
    trace(log, format!("VYGEV|net_create|{name}|{}", bit(ok)));
    Ok(ok)
}

fn inst_create(s: &mut impl EvalStore, log: &mut Log, master: &str, name: &str) -> Result<bool, Stop> {
    let ok = db(s.inst_create(master, name))?;
    trace(log, format!("VYGEV|inst_create|{name}|{master}|{}", bit(ok)));
    Ok(ok)
}

fn connect(s: &mut impl EvalStore, log: &mut Log, t: &ITerm, net: &str) -> Result<(), Stop> {
    trace(log, format!("VYGEV|connect|{}|{net}", t.name('/')));
    db(s.connect(&t.inst, &t.mterm, net))
}

fn disconnect(s: &mut impl EvalStore, log: &mut Log, t: &ITerm) -> Result<(), Stop> {
    let net = db(s.iterm_net(&t.inst, &t.mterm))?.unwrap_or_else(|| "<null>".into());
    trace(log, format!("VYGEV|disconnect|{}|{net}", t.name('/')));
    db(s.disconnect(&t.inst, &t.mterm))
}

fn add_inst(s: &mut impl EvalStore, log: &mut Log, group: &str, inst: &str) -> Result<(), Stop> {
    db(s.group_add_inst(group, inst))?;
    trace(log, format!("VYGEV|add_inst|{group}|{inst}"));
    Ok(())
}

/// The domain of a module path: the domain of the longest element that is a PREFIX of the path —
/// a plain string prefix, so `u_aes_10` matches element `u_aes_1` — else the top domain.
fn match_module_to_domain(
    module_to_domain: &mut BTreeMap<String, String>,
    path_to_domain: &PathToDomain,
    current_path: &str,
) -> String {
    let mut longest_prefix = "";
    for name in path_to_domain.keys() {
        if current_path.starts_with(name.as_str()) && name.len() > longest_prefix.len() {
            longest_prefix = name;
        }
    }
    let key = if longest_prefix.is_empty() { "." } else { longest_prefix };
    let domain = path_to_domain[key].clone();
    module_to_domain.insert(current_path.to_string(), domain.clone());
    domain
}

/// Depth first over the module tree, children in `getChildren` order: each child's domain gets the
/// domain of the module above it as parent, unless it is the top. ⚠️ A module below a domain's own
/// module matches that same domain, which then becomes its OWN parent — the last write wins.
fn build_domain_hierarchy(
    s: &mut impl EvalStore,
    log: &mut Log,
    module_to_domain: &mut BTreeMap<String, String>,
    path_to_domain: &PathToDomain,
    current_module: &str,
    current_domain: &str,
) -> Result<(), Stop> {
    for (name, child_mod) in db(s.module_children(current_module))? {
        let matched_domain = match_module_to_domain(module_to_domain, path_to_domain, &name);
        if !db(s.domain_is_top(&matched_domain))? {
            db(s.set_parent(&matched_domain, current_domain))?;
            trace(log, format!("VYGEV|set_parent|{matched_domain}|{current_domain}"));
        }
        build_domain_hierarchy(s, log, module_to_domain, path_to_domain, &child_mod, &matched_domain)?;
    }
    Ok(())
}

/// The operands of an expression node, as `FuncExpr::left` / `right` give them.
fn left(f: &FuncExpr) -> Option<&FuncExpr> {
    match f {
        FuncExpr::Not(a) | FuncExpr::Or(a, _) | FuncExpr::And(a, _) | FuncExpr::Xor(a, _) => Some(a),
        _ => None,
    }
}

fn right(f: &FuncExpr) -> Option<&FuncExpr> {
    match f {
        FuncExpr::Or(_, b) | FuncExpr::And(_, b) | FuncExpr::Xor(_, b) => Some(b),
        _ => None,
    }
}

/// `FuncExpr::hasPort`.
fn has_port(f: &FuncExpr, port: &str) -> bool {
    match f {
        FuncExpr::Port(p) => p == port,
        FuncExpr::Not(a) => has_port(a, port),
        FuncExpr::Or(a, b) | FuncExpr::And(a, b) | FuncExpr::Xor(a, b) => has_port(a, port) || has_port(b, port),
        FuncExpr::One | FuncExpr::Zero => false,
    }
}

/// Whether an isolation cell's output function can clamp as asked, and which of its control and
/// output need an inverter: `(invert_control, invert_output)`. Only the operand holding the enable
/// is looked at, and only its OWN top node for a `!`; the function's top node must be an OR or an
/// AND (an XOR included in "unknown").
fn check_isolation_match(func: &FuncExpr, enable: &str, sense: bool, clamp_val: bool, log: &mut Log) -> Option<(bool, bool)> {
    let enable_is_left = left(func).is_some_and(|f| has_port(f, enable));
    let enable_is_right = right(func).is_some_and(|f| has_port(f, enable));
    if !enable_is_left && !enable_is_right {
        log.warn("UPF", 17, "isolation cell has no enable port".into());
        return None;
    }
    let enable_func = if enable_is_left { left(func) } else { right(func) }?;
    let enable_is_inverted = matches!(enable_func, FuncExpr::Not(_));
    let new_enable_sense = if enable_is_inverted { !sense } else { sense };
    match func {
        FuncExpr::Or(..) => Some((!new_enable_sense, !clamp_val)),
        FuncExpr::And(..) => Some((new_enable_sense, clamp_val)),
        _ => {
            log.warn("UPF", 18, "unknown isolation cell function".into());
            None
        }
    }
}

/// Each domain's elements into `path_to_domain`; the domain with element "." is the top (and gets
/// no region); every other domain gets an EXCLUSIVE region bounded by its area, and a POWER_DOMAIN
/// group in it.
fn associate_groups(
    s: &mut impl EvalStore,
    log: &mut Log,
    path_to_domain: &mut PathToDomain,
    top_domain: &mut Option<String>,
) -> Result<bool, Stop> {
    for domain in db(s.power_domains())? {
        let mut is_top_domain = false;
        for el in db(s.domain_elements(&domain))? {
            if path_to_domain.contains_key(&el) {
                return Err(Stop::error("UPF", 19, format!("multiple power domain definitions for the same path {el}")));
            }
            path_to_domain.insert(el.clone(), domain.clone());
            if el == "." {
                db(s.set_top(&domain, true))?;
                trace(log, format!("VYGEV|set_top|{domain}|1"));
                *top_domain = Some(domain.clone());
                is_top_domain = true;
            }
        }
        if is_top_domain {
            continue;
        }

        let region = db(s.region_create(&domain))?;
        trace(log, format!("VYGEV|region_create|{domain}|{}", bit(region)));
        if !region {
            log.warn("UPF", 20, format!("Creation of {domain} region failed"));
            return Ok(false);
        }
        db(s.region_set_type(&domain, "EXCLUSIVE"))?;
        match db(s.domain_area(&domain))? {
            Some(a) => {
                let ok = db(s.region_add_boundary(&domain, a))?;
                trace(log, format!("VYGEV|box_create|{domain}|{}|{}|{}|{}|{}", a[0], a[1], a[2], a[3], bit(ok)));
            }
            None => log.warn("UPF", 21, format!("No area specified for {domain} power domain")),
        }

        let group = db(s.group_create(&domain, &domain))?;
        trace(log, format!("VYGEV|group_create|{domain}|{domain}|{}", bit(group)));
        if !group {
            log.warn("UPF", 22, format!("Creation of {domain} group failed, duplicate group exists."));
            return Ok(false);
        }
        db(s.group_set_type(&domain, "POWER_DOMAIN"))?;
        db(s.set_group(&domain, &domain))?;
        trace(log, format!("VYGEV|set_group|{domain}|{domain}"));
    }
    Ok(true)
}

/// A dbNet, then a dbBTerm on it, per logic port. ⚠️ The reference's `success` is sticky: after the
/// first failure every later port still gets its net tried, but no terminal.
fn instantiate_logic_ports(s: &mut impl EvalStore, log: &mut Log) -> Result<bool, Stop> {
    let mut success = true;
    for port in db(s.logic_ports())? {
        let net = db(s.net_create(&port))?;
        trace(log, format!("VYGEV|net_create|{port}|{}", bit(net)));
        if !net {
            log.warn("UPF", 23, format!("Creation of '{port}' dbNet from UPF Logic Port failed"));
            success = false;
        }
        if success {
            let term = db(s.bterm_create(&port, &port))?;
            trace(log, format!("VYGEV|bterm_create|{port}|{port}|{}", bit(term)));
            if !term {
                log.warn("UPF", 45, format!("Creation of '{port}' dbBTerm from UPF Logic Port failed"));
                success = false;
            }
        }
    }
    Ok(success)
}

/// Each instance into the group of its module's domain, when that domain has a parent (the top
/// has none, so its instances stay ungrouped). ⚠️ An instance of the top module is looked up as
/// ".", which the hierarchy walk never records: it is never grouped.
fn add_insts_to_group(
    s: &mut impl EvalStore,
    log: &mut Log,
    top_domain: &str,
    path_to_domain: &PathToDomain,
) -> Result<(), Stop> {
    let mut module_to_domain = BTreeMap::new();
    build_domain_hierarchy(s, log, &mut module_to_domain, path_to_domain, "", top_domain)?;

    for (inst, path) in db(s.inst_module_paths())? {
        if path.is_empty() {
            continue;
        }
        let Some(domain) = module_to_domain.get(&path) else {
            continue;
        };
        if db(s.domain_parent(domain))?.is_some() {
            let group = db(s.domain_group(domain))?
                .ok_or_else(|| Stop::Refused(format!("power domain {domain} has a parent but no group")))?;
            add_inst(s, log, &group, &inst)?;
        }
    }
    Ok(())
}

/// The inverter of least Liberty area over every master in library order (strictly less: the
/// first of equals), with its single input and output.
fn find_smallest_inverter(s: &impl EvalStore, lib: &Liberty) -> Result<Option<Inverter>, Stop> {
    let mut smallest_area = f32::MAX;
    let mut found = None;
    for master in db(s.masters())? {
        let Some(cell) = lib.cell(&master) else {
            continue;
        };
        if cell.is_inverter() && cell.area < smallest_area {
            smallest_area = cell.area;
            let (i, o) = cell.buffer_ports().expect("an inverter has buffer ports");
            found = Some(Inverter { master, input: i.name.clone(), output: o.name.clone() });
        }
    }
    Ok(found)
}

/// The isolation cell of least area among the strategy's cells that have a data, an enable and an
/// output terminal and whose function can clamp as asked; an inverter needed on the control or the
/// output adds the INVERTER MASTER's area, `dbMaster::getArea` in DBU² — a unit mix with the
/// Liberty area, transcribed as the reference has it.
fn find_smallest_isolation(
    s: &impl EvalStore,
    lib: &Liberty,
    log: &mut Log,
    iso_name: &str,
    iso: &IsolationRule,
    inverter: Option<&Inverter>,
) -> Result<Option<IsoCell>, Stop> {
    let isolation_sense = iso.isolation_sense == "high";
    let isolation_clamp_val = iso.clamp_value == "1";
    if iso.cells.is_empty() {
        log.warn("UPF", 24, format!("Isolation {iso_name} defined, but no cells defined."));
        return Ok(None);
    }
    let mut smallest_area = f32::MAX;
    let inverter_area = match inverter {
        Some(inv) => db(s.master_area(&inv.master))? as f32,
        None => 0.0,
    };
    let mut best = None;
    for iso_cell in &iso.cells {
        if iso_cell.is_empty() {
            return Err(Stop::Refused(format!("isolation {iso_name} names a cell that is no master (the reference dereferences null here)")));
        }
        let Some(liberty_cell) = lib.cell(iso_cell) else {
            return Err(Stop::Refused(format!("isolation cell {iso_cell} has no Liberty cell (the reference dereferences null here)")));
        };
        let (mut enable, mut enable_port, mut data, mut output, mut out_port) = (None, None, None, None, None);
        for (term, io) in db(s.master_mterms(iso_cell))? {
            let Some(lib_port) = liberty_cell.port(&term) else {
                continue;
            };
            if lib_port.is_isolation_cell_data {
                data = Some(term.clone());
            }
            if lib_port.is_isolation_cell_enable {
                enable = Some(term.clone());
                enable_port = Some(lib_port);
            }
            if io == "OUTPUT" {
                output = Some(term.clone());
                out_port = Some(lib_port);
            }
        }
        let (Some(output), Some(data), Some(enable)) = (output, data, enable) else {
            continue;
        };
        let (out_port, enable_port) = (out_port.unwrap(), enable_port.unwrap());
        let func = out_port.function.as_deref().map(FuncExpr::parse);
        let Some(Ok(func)) = func else {
            return Err(Stop::Refused(format!("isolation cell {iso_cell} output {output} has no function the reference can read")));
        };
        let Some((invert_control, invert_output)) =
            check_isolation_match(&func, &enable_port.name, isolation_sense, isolation_clamp_val, log)
        else {
            continue;
        };
        let mut tmp_area = liberty_cell.area;
        if invert_control || invert_output {
            if inverter.is_none() {
                continue;
            }
            if invert_control {
                tmp_area += inverter_area;
            }
            if invert_output {
                tmp_area += inverter_area;
            }
        }
        if tmp_area < smallest_area {
            smallest_area = tmp_area;
            best = Some(IsoCell { master: iso_cell.clone(), enable, data, output, invert_control, invert_output });
        }
    }
    if smallest_area == f32::MAX {
        log.warn("UPF", 25, format!("Isolation {iso_name} cells defined, but can't find any in the lib."));
        return Ok(None);
    }
    Ok(best)
}

/// The isolation instance `<inst>_<in net>_<out net>_isolation` (nothing when it exists already),
/// into `target_group`; an inverter on the output and/or the control as the cell needs, each with
/// its own net named after it.
#[allow(clippy::too_many_arguments)]
fn insert_isolation_cell(
    s: &mut impl EvalStore,
    log: &mut Log,
    inst: &str,
    input_net: &str,
    control_net: &str,
    output_net: &str,
    cell: &IsoCell,
    inverter: Option<&Inverter>,
    target_group: Option<&str>,
) -> Result<(), Stop> {
    let inst_name = format!("{inst}_{input_net}_{output_net}_isolation");
    if db(s.exists("inst", &inst_name))? {
        return Ok(());
    }
    inst_create(s, log, &cell.master, &inst_name)?;
    if let Some(g) = target_group {
        add_inst(s, log, g, &inst_name)?;
    }
    let at = |m: &str| ITerm { inst: inst_name.clone(), mterm: m.to_string() };
    let (enable_iterm, data_iterm, output_iterm) = (at(&cell.enable), at(&cell.data), at(&cell.output));
    let inv = || inverter.ok_or_else(|| Stop::Refused("an isolation cell needs an inverter and there is none".into()));

    if cell.invert_output {
        let inv = inv()?;
        let inv_name = format!("{inst_name}_inv_out");
        inst_create(s, log, &inv.master, &inv_name)?;
        connect(s, log, &ITerm { inst: inv_name.clone(), mterm: inv.output.clone() }, output_net)?;
        net_create(s, log, &inv_name)?;
        connect(s, log, &ITerm { inst: inv_name.clone(), mterm: inv.input.clone() }, &inv_name)?;
        connect(s, log, &output_iterm, &inv_name)?;
        if let Some(g) = target_group {
            add_inst(s, log, g, &inv_name)?;
        }
    } else {
        connect(s, log, &output_iterm, output_net)?;
    }

    if cell.invert_control {
        let inv = inv()?;
        let inv_name = format!("{inst_name}_inv_control");
        inst_create(s, log, &inv.master, &inv_name)?;
        connect(s, log, &ITerm { inst: inv_name.clone(), mterm: inv.input.clone() }, control_net)?;
        net_create(s, log, &inv_name)?;
        connect(s, log, &ITerm { inst: inv_name.clone(), mterm: inv.output.clone() }, &inv_name)?;
        connect(s, log, &enable_iterm, &inv_name)?;
        if let Some(g) = target_group {
            add_inst(s, log, g, &inv_name)?;
        }
    } else {
        connect(s, log, &enable_iterm, control_net)?;
    }

    connect(s, log, &data_iterm, input_net)
}

/// The isolation cell between `iterm`'s net and `target_iterm`, which moves to the cell's output
/// net: `<net>_<target mterm>_o` in the target's group for location `fanout`, else `<net>_o` in the
/// domain's group (`self`) or its parent's (`parent`, none when the parent is the top).
#[allow(clippy::too_many_arguments)]
fn isolate_port(
    s: &mut impl EvalStore,
    log: &mut Log,
    iterm: &ITerm,
    target_iterm: &ITerm,
    pd: &str,
    iso_name: &str,
    iso: &IsolationRule,
    cell: &IsoCell,
    inverter: Option<&Inverter>,
) -> Result<bool, Stop> {
    let Some(net) = db(s.iterm_net(&iterm.inst, &iterm.mterm))? else {
        log.warn("UPF", 71, format!("Isolation {iso_name} has no net connected"));
        return Ok(false);
    };
    if !db(s.exists("net", &iso.isolation_signal))? {
        log.warn("UPF", 27, format!("Isolation {iso_name} has nonexisting control net {}", iso.isolation_signal));
        return Ok(false);
    }
    let control_net = iso.isolation_signal.as_str();
    let (target_group, net_out_name) = if iso.location == "fanout" {
        (db(s.inst_group(&target_iterm.inst))?, format!("{net}_{}_o", target_iterm.mterm))
    } else {
        let target_group = match iso.location.as_str() {
            "parent" => match db(s.domain_parent(pd))? {
                Some(ppd) => db(s.domain_group(&ppd))?,
                None => None,
            },
            "self" => db(s.domain_group(pd))?,
            _ => {
                log.warn(
                    "UPF",
                    28,
                    format!(
                        "Isolation {iso_name} has location {}, but only self|parent|fanoutsupported, defaulting to self.",
                        iso.location
                    ),
                );
                db(s.domain_group(pd))?
            }
        };
        (target_group, format!("{net}_o"))
    };
    if !db(s.exists("net", &net_out_name))? {
        net_create(s, log, &net_out_name)?;
    }
    insert_isolation_cell(s, log, &iterm.inst, &net, control_net, &net_out_name, cell, inverter, target_group.as_deref())?;
    disconnect(s, log, target_iterm)?;
    connect(s, log, target_iterm, &net_out_name)?;
    Ok(true)
}

/// The terminals on `iterm`'s net (in the net's order) whose instance is in a different group,
/// each with the power domain named like that group (none for an ungrouped instance).
fn get_connected_terms(s: &impl EvalStore, iterm: &ITerm) -> Result<Vec<(ITerm, Option<String>)>, Stop> {
    let mut external = Vec::new();
    let Some(net) = db(s.iterm_net(&iterm.inst, &iterm.mterm))? else {
        return Ok(external);
    };
    let connected = db(s.net_iterms(&net))?;
    if connected.len() < 2 {
        return Ok(external);
    }
    let own_group = db(s.inst_group(&iterm.inst))?;
    for (inst, mterm) in connected {
        let t = ITerm { inst, mterm };
        if t == *iterm {
            continue;
        }
        let group = db(s.inst_group(&t.inst))?;
        if group != own_group {
            let domain = match group {
                Some(g) if db(s.exists("power_domain", &g))? => Some(g),
                _ => None,
            };
            external.push((t, domain));
        }
    }
    Ok(external)
}

/// The domain's FIRST isolation strategy (UPF-0030 when there are several) on one connection, when
/// it applies to the source terminal's direction.
fn isolate_connection(
    s: &mut impl EvalStore,
    lib: &Liberty,
    log: &mut Log,
    src_term: &ITerm,
    target_term: &ITerm,
    domain: &str,
) -> Result<bool, Stop> {
    let isos = db(s.domain_isolations(domain))?;
    if isos.is_empty() {
        return Ok(false);
    }
    if isos.len() > 1 {
        log.warn("UPF", 30, format!("Multiple isolation strategies defined for the same power domain {domain}."));
    }
    let iso_name = &isos[0];
    let iso = db(s.isolation(iso_name))?;
    // dbIsolation::appliesTo: an output unless "inputs", an input unless "outputs", else both.
    let applies = match io_of(s, src_term)?.as_str() {
        "OUTPUT" => iso.applies_to != "inputs",
        "INPUT" => iso.applies_to != "outputs",
        _ => true,
    };
    if !applies {
        return Ok(false);
    }
    let inverter = find_smallest_inverter(s, lib)?;
    if inverter.is_none() {
        log.warn("UPF", 31, "can't find any inverters".into());
    }
    let Some(cell) = find_smallest_isolation(s, lib, log, iso_name, &iso, inverter.as_ref())? else {
        return Ok(false);
    };
    isolate_port(s, log, src_term, target_term, domain, iso_name, &iso, &cell, inverter.as_ref())
}

/// The domain's level-shifter strategy for an instance: a `force_shift` strategy that lists it (or
/// lists nothing, or ".") and does not exclude it wins at once; a `no_shift` one that lists it
/// vetoes everything but that; otherwise the LAST listing, non-excluding, shifting strategy.
fn find_shift_strategy(s: &impl EvalStore, domain: &str, iterm: &ITerm) -> Result<Option<String>, Stop> {
    let inst = &iterm.inst;
    let mut found_strategy = None;
    let mut no_shift = false;
    for name in db(s.domain_level_shifters(domain))? {
        let shifter = db(s.level_shifter(&name))?;
        let found = shifter.elements.is_empty() || shifter.elements.iter().any(|el| el == inst || el == ".");
        let excluded = shifter.exclude_elements.iter().any(|el| el == inst || el == ".");
        if shifter.force_shift && found && !excluded {
            return Ok(Some(name));
        }
        if found && shifter.no_shift {
            no_shift = true;
        }
        if found && !excluded && !shifter.no_shift && !no_shift {
            found_strategy = Some(name);
        }
    }
    Ok(if no_shift { None } else { found_strategy })
}

/// Whether the strategy shifts this connection: both voltages known (UPF-0053), the direction
/// allowed by its rule (seen from the source terminal: an INPUT reverses it), the difference at
/// least its threshold, and the source direction within `applies_to`.
fn validate_shifting_strategy(
    s: &impl EvalStore,
    log: &mut Log,
    strategy: &ShiftRule,
    src_term: &ITerm,
    src_domain: &str,
    target_domain: &str,
) -> Result<bool, Stop> {
    let src_voltage = db(s.domain_voltage(src_domain))?;
    let target_voltage = db(s.domain_voltage(target_domain))?;
    if src_voltage == 0.0 || target_voltage == 0.0 {
        log.warn("UPF", 53, format!("Missing voltage information for one of the domains {src_domain} {target_domain}"));
        return Ok(false);
    }
    let mut low_to_high = src_voltage < target_voltage;
    let mut high_to_low = src_voltage > target_voltage;
    let io = io_of(s, src_term)?;
    if io == "INPUT" {
        low_to_high = !low_to_high;
        high_to_low = !high_to_low;
    }
    let difference = (src_voltage - target_voltage).abs();
    if low_to_high && strategy.rule != "low_to_high" && strategy.rule != "both" {
        return Ok(false);
    }
    if high_to_low && strategy.rule != "high_to_low" && strategy.rule != "both" {
        return Ok(false);
    }
    if difference < strategy.threshold {
        return Ok(false);
    }
    if io == "INPUT" && strategy.applies_to != "inputs" && strategy.applies_to != "both" {
        return Ok(false);
    }
    if io == "OUTPUT" && strategy.applies_to != "outputs" && strategy.applies_to != "both" {
        return Ok(false);
    }
    Ok(true)
}

/// The level shifter `<iterm>_<target iterm>_level_shifter` (names joined by `_`) on the source's
/// net, its output on `<that>_out_net`, which the input terminal (for an input source) or the
/// target (for an output source) moves to.
#[allow(clippy::too_many_arguments)]
fn insert_level_shifter(
    s: &mut impl EvalStore,
    log: &mut Log,
    iterm: &ITerm,
    target_iterm: &ITerm,
    pd: &str,
    target_domain: &str,
    shift_name: &str,
    shift: &ShiftRule,
) -> Result<bool, Stop> {
    let net = db(s.iterm_net(&iterm.inst, &iterm.mterm))?.unwrap_or_default();
    if !db(s.exists("master", &shift.cell_name))? {
        log.warn("UPF", 54, format!("Can't find master {} for level shifter {shift_name}", shift.cell_name));
        return Ok(false);
    }
    let inst_name = format!("{}_{}_level_shifter", iterm.name('_'), target_iterm.name('_'));
    if !inst_create(s, log, &shift.cell_name, &inst_name)? {
        return Err(Stop::Refused(format!("level shifter {inst_name} exists already (the reference dereferences null here)")));
    }
    let terms = db(s.master_mterms(&shift.cell_name))?;
    let has = |m: &str| terms.iter().any(|(t, _)| t == m);
    if !has(&shift.cell_input) || !has(&shift.cell_output) {
        log.warn(
            "UPF",
            55,
            format!(
                "Can't find input or output term for level shifter {shift_name} based on strategy specified input {} output {}",
                shift.cell_input, shift.cell_output
            ),
        );
        return Ok(false);
    }
    let target_group = match shift.location.as_str() {
        "parent" => match db(s.domain_parent(pd))? {
            Some(ppd) => db(s.domain_group(&ppd))?,
            None => None,
        },
        "self" => db(s.domain_group(pd))?,
        "fanout" => db(s.domain_group(target_domain))?,
        _ => {
            log.warn(
                "UPF",
                35,
                format!(
                    "Level Shifting strategy {shift_name} has location {}, but only self|parent|fanoutsupported, defaulting to self.",
                    shift.location
                ),
            );
            db(s.domain_group(pd))?
        }
    };
    let Some(target_group) = target_group else {
        return Err(Stop::Refused(format!("level shifter {inst_name} has no group to go in (the reference dereferences null here)")));
    };
    add_inst(s, log, &target_group, &inst_name)?;

    let net_out_name = format!("{inst_name}_out_net");
    if !db(s.exists("net", &net_out_name))? {
        net_create(s, log, &net_out_name)?;
    }
    connect(s, log, &ITerm { inst: inst_name.clone(), mterm: shift.cell_input.clone() }, &net)?;
    connect(s, log, &ITerm { inst: inst_name.clone(), mterm: shift.cell_output.clone() }, &net_out_name)?;
    match io_of(s, iterm)?.as_str() {
        "INPUT" => {
            disconnect(s, log, iterm)?;
            connect(s, log, iterm, &net_out_name)?;
        }
        "OUTPUT" => {
            disconnect(s, log, target_iterm)?;
            connect(s, log, target_iterm, &net_out_name)?;
        }
        _ => {
            log.warn(
                "UPF",
                56,
                format!("Level Shifting strategy {shift_name} has unknown io type, but only input|outputsupported."),
            );
            return Ok(false);
        }
    }
    Ok(true)
}

/// Evaluate the power intent in the database. `Ok(false)` where the reference returns false (a
/// region or group could not be created; the caller carries on); an error the reference raises
/// stops the evaluation.
pub fn eval_upf(s: &mut impl EvalStore, lib: &Liberty, log: &mut Log) -> Result<bool, Stop> {
    instantiate_logic_ports(s, log)?;
    let pds = db(s.power_domains())?;
    if pds.is_empty() {
        return Ok(true);
    }
    let mut path_to_domain = PathToDomain::new();
    let mut top_domain = None;
    if !associate_groups(s, log, &mut path_to_domain, &mut top_domain)? {
        return Ok(false);
    }
    let Some(top_domain) = top_domain else {
        return Err(Stop::error("UPF", 29, "No TOP DOMAIN found, aborting"));
    };
    add_insts_to_group(s, log, &top_domain, &path_to_domain)?;

    // A level shifter's cell is held in memory only (it is not stored in the database): read from
    // a database, every one is empty, and the reference — which still has them — would differ.
    let shifter_cells = db(s.level_shifter_cells())?;
    if shifter_cells.iter().any(String::is_empty) {
        return Err(Stop::Refused(
            "a level shifter has no cell: its cell is not stored in the database, so it is lost unless the UPF is read in the same run".into(),
        ));
    }
    let level_shifter_cells: HashMap<String, bool> = shifter_cells.into_iter().map(|c| (c, true)).collect();
    let is_shifter = |master: &str| level_shifter_cells.get(master).copied().unwrap_or(false);

    for domain in &pds {
        if *domain == top_domain {
            continue;
        }
        let curr_group =
            db(s.domain_group(domain))?.ok_or_else(|| Stop::Refused(format!("power domain {domain} has no group")))?;
        for inst in db(s.group_insts(&curr_group))? {
            if is_shifter(&db(s.inst_master(&inst))?) {
                continue;
            }
            for (mterm, _) in db(s.inst_iterms(&inst))? {
                let iterm = ITerm { inst: inst.clone(), mterm };
                for (target_iterm, target_domain) in get_connected_terms(s, &iterm)? {
                    if is_shifter(&db(s.inst_master(&target_iterm.inst))?) {
                        continue;
                    }
                    let same_voltage = match &target_domain {
                        None => true,
                        Some(t) => db(s.domain_voltage(domain))? == db(s.domain_voltage(t))?,
                    };
                    if same_voltage {
                        isolate_connection(s, lib, log, &iterm, &target_iterm, domain)?;
                        continue;
                    }
                    let target_domain = target_domain.unwrap();
                    let Some(strategy_name) = find_shift_strategy(s, domain, &iterm)? else {
                        continue;
                    };
                    let strategy = db(s.level_shifter(&strategy_name))?;
                    if !validate_shifting_strategy(s, log, &strategy, &iterm, domain, &target_domain)? {
                        continue;
                    }
                    insert_level_shifter(s, log, &iterm, &target_iterm, domain, &target_domain, &strategy_name, &strategy)?;
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
