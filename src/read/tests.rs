// SPDX-License-Identifier: Apache-2.0
//! The reader against an in-memory store that keeps the intent in [`PowerIntent`], so a read can
//! be written back with the real writer. Each test pins one rule of the reference's front end or
//! database layer; the rule is stated at the test.

use super::*;
use crate::{Domain, Isolation, LevelShifter, LogicPort, PowerIntent, PowerSwitch};
use std::collections::BTreeMap;

#[derive(Default)]
struct Mem {
    pi: PowerIntent,
    /// cell -> its terminals, in library order
    masters: Vec<(String, Vec<String>)>,
    switches_of: BTreeMap<String, Vec<String>>,
    isolations_of: BTreeMap<String, Vec<String>>,
    area: BTreeMap<String, [i32; 4]>,
    voltage: BTreeMap<String, f32>,
    iso_cells: BTreeMap<String, Vec<String>>,
    ls_cell: BTreeMap<String, [String; 3]>,
}

impl Mem {
    fn new() -> Mem {
        Mem { masters: vec![("and2".into(), vec!["A".into(), "B".into(), "X".into()])], ..Default::default() }
    }
    fn sw(&mut self, n: &str) -> &mut PowerSwitch {
        self.pi.switches.iter_mut().find(|s| s.name == n).unwrap()
    }
    fn iso(&mut self, n: &str) -> &mut Isolation {
        self.pi.isolations.iter_mut().find(|s| s.name == n).unwrap()
    }
    fn ls(&mut self, n: &str) -> &mut LevelShifter {
        self.pi.level_shifters.iter_mut().find(|s| s.name == n).unwrap()
    }
}

impl UpfStore for Mem {
    fn has_chip(&self) -> bool {
        true
    }
    fn has_block(&self) -> bool {
        true
    }
    fn find(&self, kind: &str, name: &str) -> Result<bool, String> {
        let p = &self.pi;
        Ok(match kind {
            "power_domain" => p.domains.iter().any(|d| d.name == name),
            "logic_port" => p.ports.iter().any(|d| d.name == name),
            "power_switch" => p.switches.iter().any(|d| d.name == name),
            "isolation" => p.isolations.iter().any(|d| d.name == name),
            "level_shifter" => p.level_shifters.iter().any(|d| d.name == name),
            "master" => self.masters.iter().any(|(m, _)| m == name),
            k => return Err(format!("unknown kind {k}")),
        })
    }
    fn dbu_per_micron(&self) -> Result<i32, String> {
        Ok(1000)
    }
    fn pwr_domain_create(&mut self, name: &str) -> Result<bool, String> {
        if self.find("power_domain", name)? {
            return Ok(false);
        }
        self.pi.domains.push(Domain { name: name.into(), ..Default::default() });
        Ok(true)
    }
    fn pwr_domain_add_element(&mut self, pd: &str, e: &str) -> Result<(), String> {
        self.pi.domains.iter_mut().find(|d| d.name == pd).unwrap().elements.push(e.into());
        Ok(())
    }
    fn pwr_domain_add_power_switch(&mut self, pd: &str, sw: &str) -> Result<(), String> {
        self.switches_of.entry(pd.into()).or_default().push(sw.into());
        Ok(())
    }
    fn pwr_domain_add_isolation(&mut self, pd: &str, iso: &str) -> Result<(), String> {
        self.isolations_of.entry(pd.into()).or_default().push(iso.into());
        Ok(())
    }
    fn pwr_domain_set_area(&mut self, pd: &str, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<(), String> {
        self.area.insert(pd.into(), [x1, y1, x2, y2]);
        Ok(())
    }
    fn pwr_domain_set_voltage(&mut self, pd: &str, v: f32) -> Result<(), String> {
        self.voltage.insert(pd.into(), v);
        Ok(())
    }
    fn logic_port_create(&mut self, name: &str, direction: &str) -> Result<bool, String> {
        if self.find("logic_port", name)? {
            return Ok(false);
        }
        let direction = if direction.is_empty() { "in" } else { direction };
        self.pi.ports.push(LogicPort { name: name.into(), direction: direction.into() });
        Ok(true)
    }
    fn pwr_switch_create(&mut self, name: &str) -> Result<bool, String> {
        if self.find("power_switch", name)? {
            return Ok(false);
        }
        self.pi.switches.push(PowerSwitch { name: name.into(), ..Default::default() });
        Ok(true)
    }
    fn pwr_switch_set_power_domain(&mut self, sw: &str, pd: &str) -> Result<(), String> {
        self.sw(sw).domain = pd.into();
        Ok(())
    }
    fn pwr_switch_set_out_supply_port(&mut self, sw: &str, port: &str, net: &str) -> Result<(), String> {
        self.sw(sw).output = (port.into(), net.into());
        Ok(())
    }
    fn pwr_switch_add_in_supply_port(&mut self, sw: &str, port: &str, net: &str) -> Result<(), String> {
        self.sw(sw).inputs.push((port.into(), net.into()));
        Ok(())
    }
    fn pwr_switch_add_control_port(&mut self, sw: &str, port: &str, net: &str) -> Result<(), String> {
        self.sw(sw).controls.push((port.into(), net.into()));
        Ok(())
    }
    fn pwr_switch_add_acknowledge_port(&mut self, sw: &str, port: &str, net: &str, e: &str) -> Result<(), String> {
        self.sw(sw).acks.push((port.into(), net.into(), e.into()));
        Ok(())
    }
    fn pwr_switch_add_on_state(&mut self, sw: &str, st: &str, port: &str, e: &str) -> Result<(), String> {
        self.sw(sw).on_states.push((st.into(), port.into(), e.into()));
        Ok(())
    }
    fn pwr_switch_set_lib_cell(&mut self, sw: &str, master: &str) -> Result<(), String> {
        self.sw(sw).lib_cell = Some(master.into());
        Ok(())
    }
    /// As odb: the switch port must be a terminal of the mapped cell; the map is keyed (and kept
    /// sorted) by model port, a repeat overwriting.
    fn pwr_switch_add_port_map(&mut self, sw: &str, model: &str, port: &str) -> Result<(), String> {
        let Some(cell) = self.sw(sw).lib_cell.clone() else {
            return Err(format!("ODB-0153: Cannot map port {model} to {port} because no lib cell is added"));
        };
        let terms = &self.masters.iter().find(|(m, _)| *m == cell).unwrap().1;
        if !terms.iter().any(|t| t == port) {
            return Err(format!("ODB-0154: Cannot map port {model} to {port} because the mterm is not found"));
        }
        let pm = &mut self.sw(sw).port_map;
        pm.retain(|(m, _)| m != model);
        pm.push((model.into(), port.into()));
        pm.sort();
        Ok(())
    }
    fn isolation_create(&mut self, name: &str) -> Result<bool, String> {
        if self.find("isolation", name)? {
            return Ok(false);
        }
        self.pi.isolations.push(Isolation { name: name.into(), ..Default::default() });
        Ok(true)
    }
    fn isolation_set_power_domain(&mut self, iso: &str, pd: &str) -> Result<(), String> {
        self.iso(iso).domain = pd.into();
        Ok(())
    }
    fn isolation_set(&mut self, iso: &str, field: &str, v: &str) -> Result<(), String> {
        let i = self.iso(iso);
        *match field {
            "applies_to" => &mut i.applies_to,
            "clamp_value" => &mut i.clamp_value,
            "isolation_signal" => &mut i.isolation_signal,
            "isolation_sense" => &mut i.isolation_sense,
            "location" => &mut i.location,
            f => return Err(format!("unknown isolation field {f}")),
        } = v.into();
        Ok(())
    }
    fn isolation_add_isolation_cell(&mut self, iso: &str, cell: &str) -> Result<(), String> {
        self.iso_cells.entry(iso.into()).or_default().push(cell.into());
        Ok(())
    }
    fn level_shifter_create(&mut self, name: &str, pd: &str) -> Result<bool, String> {
        if self.find("level_shifter", name)? || !self.find("power_domain", pd)? {
            return Ok(false);
        }
        self.pi.level_shifters.push(LevelShifter { name: name.into(), domain: pd.into(), ..Default::default() });
        Ok(true)
    }
    fn level_shifter_set(&mut self, ls: &str, field: &str, v: &str) -> Result<(), String> {
        if field.starts_with("cell_") {
            let e = self.ls_cell.entry(ls.into()).or_default();
            let k = ["cell_name", "cell_input", "cell_output"].iter().position(|f| *f == field).unwrap();
            e[k] = v.into();
            return Ok(());
        }
        let l = self.ls(ls);
        *match field {
            "source" => &mut l.source,
            "sink" => &mut l.sink,
            "applies_to" => &mut l.applies_to,
            "applies_to_boundary" => &mut l.applies_to_boundary,
            "rule" => &mut l.rule,
            "location" => &mut l.location,
            "input_supply" => &mut l.input_supply,
            "output_supply" => &mut l.output_supply,
            "internal_supply" => &mut l.internal_supply,
            "name_prefix" => &mut l.name_prefix,
            "name_suffix" => &mut l.name_suffix,
            f => return Err(format!("unknown level shifter field {f}")),
        } = v.into();
        Ok(())
    }
    fn level_shifter_set_threshold(&mut self, ls: &str, v: f32) -> Result<(), String> {
        self.ls(ls).threshold = v;
        Ok(())
    }
    fn level_shifter_set_bool(&mut self, ls: &str, field: &str, v: bool) -> Result<(), String> {
        let l = self.ls(ls);
        *match field {
            "no_shift" => &mut l.no_shift,
            "force_shift" => &mut l.force_shift,
            "use_functional_equivalence" => &mut l.use_functional_equivalence,
            f => return Err(format!("unknown level shifter flag {f}")),
        } = v;
        Ok(())
    }
    fn level_shifter_add_element(&mut self, ls: &str, e: &str) -> Result<(), String> {
        self.ls(ls).elements.push(e.into());
        Ok(())
    }
    fn level_shifter_add_exclude_element(&mut self, ls: &str, e: &str) -> Result<(), String> {
        self.ls(ls).exclude_elements.push(e.into());
        Ok(())
    }
    fn level_shifter_add_instance(&mut self, ls: &str, inst: &str, port: &str) -> Result<(), String> {
        self.ls(ls).instances.push((inst.into(), port.into()));
        Ok(())
    }
}

fn read(src: &str) -> (Mem, Log, Result<usize, Stop>) {
    let mut m = Mem::new();
    let mut log = Log { trace: true, ..Default::default() };
    let r = read_upf(&mut m, src, &mut log);
    (m, log, r)
}

fn lines(log: &Log) -> Vec<&str> {
    log.lines.iter().map(String::as_str).collect()
}

/// A power switch: the output port is padded to two fields, each repeatable option is one call per
/// occurrence, and the calls come in the proc's order (output, inputs, controls, acks, on states)
/// whatever order the file gives them in.
#[test]
fn a_power_switch_makes_the_procs_calls_in_the_procs_order() {
    let (m, log, r) = read(
        "create_power_domain PD -elements {u1 u2}\n\
         create_power_switch sw -on_state {ON vin {!sleep}} -domain PD -control_port {sleep ctl} \\\n\
           -input_supply_port {vin VDD} -output_supply_port {vout} -input_supply_port {vin2}\n",
    );
    assert_eq!(r, Ok(2));
    assert_eq!(
        lines(&log),
        [
            "VYG-UPF create_power_domain_cmd <PD>",
            "VYG-UPF update_power_domain_cmd <PD> <u1>",
            "VYG-UPF update_power_domain_cmd <PD> <u2>",
            "VYG-UPF create_power_switch_cmd <sw> <PD>",
            "VYG-UPF update_power_switch_output_cmd <sw> <vout> <>",
            "VYG-UPF update_power_switch_input_cmd <sw> <vin> <VDD>",
            "VYG-UPF update_power_switch_input_cmd <sw> <vin2> <>",
            "VYG-UPF update_power_switch_control_cmd <sw> <sleep> <ctl>",
            "VYG-UPF update_power_switch_on_cmd <sw> <ON> <vin> <!sleep>",
        ]
    );
    assert_eq!(m.switches_of["PD"], ["sw"]);
    assert_eq!(m.pi.switches[0].domain, "PD");
}

/// `set_isolation` without `-update` defaults the sense to `high` and the location to `self`, and
/// joins the domain; with `-update` it passes empty fields and leaves the domain alone. An update of
/// a strategy that does not exist is UPF-0013 and changes nothing.
#[test]
fn isolation_defaults_apply_only_when_creating() {
    let (m, log, r) = read(
        "create_power_domain PD\n\
         set_isolation iso -domain PD -applies_to outputs\n\
         set_isolation iso -domain PD -update -isolation_signal en -isolation_sense low\n\
         set_isolation nope -domain PD -update\n",
    );
    assert_eq!(r, Ok(4));
    assert_eq!(
        lines(&log)[1..],
        [
            "VYG-UPF set_isolation_cmd <iso> <PD> <0> <outputs> <> <> <high> <self>",
            "VYG-UPF set_isolation_cmd <iso> <PD> <1> <> <> <en> <low> <>",
            "VYG-UPF set_isolation_cmd <nope> <PD> <1> <> <> <> <> <>",
            "[WARNING UPF-0013] Couldn't update a non existing isolation nope",
        ]
    );
    let i = &m.pi.isolations[0];
    assert_eq!((i.applies_to.as_str(), i.isolation_sense.as_str(), i.location.as_str(), i.isolation_signal.as_str()), ("outputs", "low", "self", "en"));
    assert_eq!(m.isolations_of["PD"], ["iso"], "an update does not join the domain again");
}

/// The reference's `set_level_shifter` tests a call that returns nothing against 0, so it never
/// returns early: a strategy that could not be created still gets one call per element, each
/// warning UPF-0039 on its own.
#[test]
fn level_shifter_elements_are_tried_even_when_the_create_failed() {
    let (_, log, r) = read("set_level_shifter ls -domain NOPE -elements {a b} -threshold 2.1\n");
    assert_eq!(r, Ok(1));
    assert_eq!(
        lines(&log),
        [
            "VYG-UPF create_or_update_level_shifter_cmd <ls> <NOPE> <> <> <TRUE> <> <> <> <2.1> <> <> <> <> <> <> <> <> <0>",
            "[WARNING UPF-0038] Couldn't find power domain NOPE for level shifter ls",
            "VYG-UPF add_level_shifter_element_cmd <ls> <a>",
            "[WARNING UPF-0039] Couldn't find level shifter ls to add element a",
            "VYG-UPF add_level_shifter_element_cmd <ls> <b>",
            "[WARNING UPF-0039] Couldn't find level shifter ls to add element b",
        ]
    );
}

/// `-use_equivalence` warns UPF-0057 from the proc, BEFORE its call; `-no_shift` passes "1";
/// `-use_functional_equivalence` is compared upper-cased with TRUE; the threshold is read as
/// `std::stof` reads it; an `-update` of a missing strategy is UPF-0032.
#[test]
fn level_shifter_flags_and_their_order() {
    let (m, log, r) = read(
        "create_power_domain PD\n\
         set_level_shifter ls2 -domain PD -update\n\
         set_level_shifter ls -domain PD -no_shift -use_functional_equivalence true -threshold 0.0625x -use_equivalence 1 \\\n\
           -instance {{i1 p1} {i2 p2}} -exclude_elements {x}\n",
    );
    assert_eq!(r, Ok(3));
    assert_eq!(lines(&log)[2], "[WARNING UPF-0032] Couldn't find level shifter ls2 to update");
    assert_eq!(lines(&log)[3], "[WARNING UPF-0057] -use_equivalence is deprecated in UPF and not supported");
    assert!(lines(&log)[4].starts_with("VYG-UPF create_or_update_level_shifter_cmd <ls> <PD> <> <> <true>"), "{log:?}");
    let l = &m.pi.level_shifters[0];
    assert!(l.no_shift && !l.force_shift && l.use_functional_equivalence);
    assert_eq!(l.threshold, 0.0625);
    assert_eq!(l.instances, [("i1".to_string(), "p1".to_string()), ("i2".to_string(), "p2".to_string())]);
    assert_eq!(l.exclude_elements, ["x"]);
}

/// `map_power_switch` takes the switches from its POSITIONAL argument (`-switch_name_list` is
/// parsed and ignored), the i-th cell for the i-th switch, and the whole port map for each. The map
/// is keyed by model port. A switch with no i-th cell asks for master "" and stops the read
/// (UPF-0043, an error).
#[test]
fn map_power_switch_reads_the_positional_list() {
    let src = "create_power_domain PD\n\
               create_power_switch a -domain PD\n\
               create_power_switch b -domain PD\n\
               map_power_switch {a b} -switch_name_list {ignored} -lib_cells {and2 and2} -port_map {{vout X} {vin A} {vout B}}\n";
    let (m, log, r) = read(src);
    assert_eq!(r, Ok(4));
    assert_eq!(lines(&log).iter().filter(|l| l.contains("set_power_switch_cell")).count(), 2);
    let pm: Vec<(&str, &str)> = m.pi.switches[1].port_map.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(pm, [("vin", "A"), ("vout", "B")], "by model port; the later vout wins");
    let (_, _, r) = read(&src.replace("{and2 and2}", "{and2}"));
    assert_eq!(r.unwrap_err().to_string(), "[ERROR UPF-0043] Cannot find master ");
}

/// Keys match by unique PREFIX as the shared Tcl parser does; an unknown key is STA-0562 and stops
/// the read, so commands after it are not applied.
#[test]
fn a_prefix_key_works_and_an_unknown_key_stops_the_read() {
    let (m, _, r) = read("create_power_domain PD -elem {.}\ncreate_logic_port p -dir out\ncreate_logic_port q -bogus 1\ncreate_logic_port z\n");
    assert_eq!(r.unwrap_err().to_string(), "[ERROR STA-0562] create_logic_port -bogus is not a known keyword or flag.");
    assert_eq!(m.pi.domains[0].elements, ["."]);
    assert_eq!(m.pi.ports.iter().map(|p| (p.name.as_str(), p.direction.as_str())).collect::<Vec<_>>(), [("p", "out")]);
}

/// Area in microns becomes DBU as `std::round(um * dbu)`; four coordinates or UPF-0036; no area is
/// UPF-0037; a negative coordinate is STA-0572.
#[test]
fn domain_area_is_rounded_to_dbu() {
    let (m, log, r) = read("create_power_domain PD\nset_domain_area PD -area {27.0004 27 60.0005 60}\n");
    assert_eq!(r, Ok(2));
    assert_eq!(m.area["PD"], [27000, 27000, 60001, 60000]);
    assert_eq!(lines(&log)[1], "VYG-UPF set_domain_area_cmd <PD> <27000 27000 60001 60000>");
    for (area, err) in [
        ("-area {1 2 3}", "[ERROR UPF-0036] -area is a list of 4 coordinates"),
        ("", "[ERROR UPF-0037] please define area"),
        ("-area {1 -2 3 4}", "[ERROR STA-0572] -area '-2' is not a positive float."),
    ] {
        let (_, _, r) = read(&format!("create_power_domain PD\nset_domain_area PD {area}\n"));
        assert_eq!(r.unwrap_err().to_string(), err, "{area}");
    }
}

/// A duplicate name is a warning, not an error: the reference's `create` returns nothing and the
/// read goes on.
#[test]
fn a_duplicate_is_a_warning_and_the_read_goes_on() {
    let (m, log, r) = read("create_power_domain PD\ncreate_power_domain PD\ncreate_logic_port p\ncreate_logic_port p\nset_domain_voltage -domain PD -voltage 1.8\n");
    assert_eq!(r, Ok(5));
    let w: Vec<&str> = lines(&log).into_iter().filter(|l| l.starts_with("[WARNING")).collect();
    assert_eq!(w, ["[WARNING UPF-0001] Creation of PD power domain failed", "[WARNING UPF-0003] Creation of p logic port failed"]);
    assert_eq!(m.voltage["PD"], 1.8);
}

/// Tcl this reader does not evaluate is refused, never guessed at.
#[test]
fn other_tcl_is_refused() {
    for src in ["set pd PD_TOP\n", "create_power_domain $pd\n", "foreach d {a b} { create_power_domain $d }\n"] {
        let (_, _, r) = read(src);
        assert!(matches!(r, Err(Stop::Refused(_))), "{src}: {r:?}");
    }
}

/// Read, then write with the real writer: what the file declared comes back in declaration order.
#[test]
fn a_read_writes_back_in_declaration_order() {
    let (m, _, r) = read(
        "create_power_domain PD_Z -elements {u2}\ncreate_power_domain PD_A -elements {u1}\n\
         create_logic_port zz -direction out\ncreate_logic_port aa\n",
    );
    assert_eq!(r, Ok(4));
    let text = crate::write_upf(&m.pi);
    let z = text.find("create_power_domain PD_Z").unwrap();
    let a = text.find("create_power_domain PD_A").unwrap();
    assert!(z < a, "{text}");
    assert!(text.find("create_logic_port zz").unwrap() < text.find("create_logic_port aa").unwrap());
}

/// The database's own error (a port map naming a terminal the cell does not have) stops the read
/// as an ERROR carrying the database's message id, not as a refusal.
#[test]
fn a_database_error_stops_the_read_as_an_error() {
    let (_, _, r) = read("create_power_domain PD\ncreate_power_switch sw -domain PD\nmap_power_switch sw -lib_cells {and2} -port_map {{vin NOPE}}\n");
    let e = r.unwrap_err();
    assert!(matches!(&e, Stop::Database { tool, id: 154, .. } if tool == "ODB"), "{e:?}");
    assert_eq!(e.to_string(), "[ERROR ODB-0154] Cannot map port vin to NOPE because the mterm is not found");
}
