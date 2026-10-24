// SPDX-License-Identifier: Apache-2.0
//! The evaluation against an in-memory store that keeps odb's rules for the calls it makes: a
//! taken name refuses a create, a group's instances are prepended, a module's children are listed
//! as given. The first five tests are the reference's own unit tests for this step (a top domain
//! and one domain per sub-block); the rest pin rules its regressions never reach. Each states the
//! rule it pins.

use super::*;

#[derive(Default, Clone)]
struct Pd {
    name: String,
    elements: Vec<String>,
    area: Option<[i32; 4]>,
    top: bool,
    parent: Option<String>,
    group: Option<String>,
}

/// A master: (name, mterms (name, io), area in DBU²).
type MasterRow = (String, Vec<(String, String)>, i64);

#[derive(Default)]
struct Mem {
    ports: Vec<String>,
    nets: Vec<String>,
    bterms: Vec<String>,
    pds: Vec<Pd>,
    /// module ("" = top) -> children in getChildren order: (hierarchical name, master module)
    children: BTreeMap<String, Vec<(String, String)>>,
    /// instances in block order with their module path
    insts: Vec<(String, String)>,
    /// region -> (type, boundaries)
    regions: Vec<(String, String, Vec<[i32; 4]>)>,
    /// group -> (region, type, instances in the group's own order)
    groups: Vec<(String, String, String, Vec<String>)>,
    voltage: BTreeMap<String, f32>,
    isolations_of: BTreeMap<String, Vec<String>>,
    shifters_of: BTreeMap<String, Vec<String>>,
    isolations: BTreeMap<String, IsolationRule>,
    shifters: BTreeMap<String, ShiftRule>,
    /// masters in library order: (name, mterms (name, io), area in DBU²)
    masters: Vec<MasterRow>,
    /// instance -> master, in creation order
    inst_masters: Vec<(String, String)>,
    /// (instance, mterm) -> net
    iterm_nets: BTreeMap<(String, String), String>,
    /// net -> its terminals, newest first (odb prepends on connect)
    net_terms: BTreeMap<String, Vec<(String, String)>>,
}

impl Mem {
    fn pd(&mut self, n: &str) -> &mut Pd {
        self.pds.iter_mut().find(|p| p.name == n).unwrap()
    }
    fn get(&self, n: &str) -> &Pd {
        self.pds.iter().find(|p| p.name == n).unwrap()
    }
    fn group(&self, n: &str) -> Option<&(String, String, String, Vec<String>)> {
        self.groups.iter().find(|g| g.0 == n)
    }
    fn inst_group(&self, inst: &str) -> Option<&str> {
        self.groups.iter().find(|g| g.3.iter().any(|i| i == inst)).map(|g| g.0.as_str())
    }
    fn domain(&mut self, name: &str, element: &str) {
        self.pds.push(Pd { name: name.into(), elements: vec![element.into()], ..Default::default() });
    }
    fn master_of(&self, m: &str) -> &MasterRow {
        self.masters.iter().find(|x| x.0 == m).unwrap()
    }
    /// A leaf instance of `master` in module `path`, its terminals on the given nets.
    fn leaf(&mut self, name: &str, master: &str, path: &str, nets: &[(&str, &str)]) {
        self.inst_masters.push((name.into(), master.into()));
        self.insts.push((name.into(), path.into()));
        for (m, n) in nets {
            self.connect(name, m, n).unwrap();
        }
    }
}

impl EvalStore for Mem {
    fn logic_ports(&self) -> Result<Vec<String>, String> {
        Ok(self.ports.clone())
    }
    fn power_domains(&self) -> Result<Vec<String>, String> {
        Ok(self.pds.iter().map(|p| p.name.clone()).collect())
    }
    fn domain_elements(&self, pd: &str) -> Result<Vec<String>, String> {
        Ok(self.get(pd).elements.clone())
    }
    fn domain_area(&self, pd: &str) -> Result<Option<[i32; 4]>, String> {
        Ok(self.get(pd).area)
    }
    fn domain_is_top(&self, pd: &str) -> Result<bool, String> {
        Ok(self.get(pd).top)
    }
    fn domain_parent(&self, pd: &str) -> Result<Option<String>, String> {
        Ok(self.get(pd).parent.clone())
    }
    fn domain_group(&self, pd: &str) -> Result<Option<String>, String> {
        Ok(self.get(pd).group.clone())
    }
    fn domain_voltage(&self, pd: &str) -> Result<f32, String> {
        Ok(self.voltage.get(pd).copied().unwrap_or(0.0))
    }
    fn domain_isolations(&self, pd: &str) -> Result<Vec<String>, String> {
        Ok(self.isolations_of.get(pd).cloned().unwrap_or_default())
    }
    fn domain_level_shifters(&self, pd: &str) -> Result<Vec<String>, String> {
        Ok(self.shifters_of.get(pd).cloned().unwrap_or_default())
    }
    fn group_insts(&self, group: &str) -> Result<Vec<String>, String> {
        Ok(self.group(group).unwrap().3.clone())
    }
    fn inst_master(&self, inst: &str) -> Result<String, String> {
        // An instance the fixture gave no master (the grouping tests) has no terminals.
        Ok(self.inst_masters.iter().find(|(i, _)| i == inst).map(|x| x.1.clone()).unwrap_or_default())
    }
    fn inst_group(&self, inst: &str) -> Result<Option<String>, String> {
        Ok(self.inst_group(inst).map(str::to_string))
    }
    fn inst_iterms(&self, inst: &str) -> Result<Vec<(String, String)>, String> {
        let m = self.inst_master(inst)?;
        Ok(self.masters.iter().find(|x| x.0 == m).map(|x| x.1.clone()).unwrap_or_default())
    }
    fn iterm_net(&self, inst: &str, mterm: &str) -> Result<Option<String>, String> {
        Ok(self.iterm_nets.get(&(inst.to_string(), mterm.to_string())).cloned())
    }
    fn net_iterms(&self, net: &str) -> Result<Vec<(String, String)>, String> {
        Ok(self.net_terms.get(net).cloned().unwrap_or_default())
    }
    fn level_shifter_cells(&self) -> Result<Vec<String>, String> {
        Ok(self.shifters.values().map(|s| s.cell_name.clone()).collect())
    }
    fn exists(&self, kind: &str, name: &str) -> Result<bool, String> {
        Ok(match kind {
            "power_domain" => self.pds.iter().any(|p| p.name == name),
            "inst" => self.inst_masters.iter().any(|(i, _)| i == name),
            "net" => self.nets.iter().any(|n| n == name),
            "master" => self.masters.iter().any(|m| m.0 == name),
            k => return Err(format!("unknown kind {k}")),
        })
    }
    fn isolation(&self, iso: &str) -> Result<IsolationRule, String> {
        Ok(self.isolations[iso].clone())
    }
    fn level_shifter(&self, ls: &str) -> Result<ShiftRule, String> {
        Ok(self.shifters[ls].clone())
    }
    fn masters(&self) -> Result<Vec<String>, String> {
        Ok(self.masters.iter().map(|m| m.0.clone()).collect())
    }
    fn master_mterms(&self, master: &str) -> Result<Vec<(String, String)>, String> {
        Ok(self.master_of(master).1.clone())
    }
    fn master_area(&self, master: &str) -> Result<i64, String> {
        Ok(self.master_of(master).2)
    }
    fn module_children(&self, module: &str) -> Result<Vec<(String, String)>, String> {
        Ok(self.children.get(module).cloned().unwrap_or_default())
    }
    fn inst_module_paths(&self) -> Result<Vec<(String, String)>, String> {
        Ok(self.insts.clone())
    }
    fn net_create(&mut self, name: &str) -> Result<bool, String> {
        if self.nets.iter().any(|n| n == name) {
            return Ok(false);
        }
        self.nets.push(name.into());
        Ok(true)
    }
    fn bterm_create(&mut self, _net: &str, name: &str) -> Result<bool, String> {
        if self.bterms.iter().any(|n| n == name) {
            return Ok(false);
        }
        self.bterms.push(name.into());
        Ok(true)
    }
    fn set_top(&mut self, pd: &str, top: bool) -> Result<(), String> {
        self.pd(pd).top = top;
        Ok(())
    }
    fn set_parent(&mut self, pd: &str, parent: &str) -> Result<(), String> {
        self.pd(pd).parent = Some(parent.into());
        Ok(())
    }
    fn set_group(&mut self, pd: &str, group: &str) -> Result<(), String> {
        self.pd(pd).group = Some(group.into());
        Ok(())
    }
    fn region_create(&mut self, name: &str) -> Result<bool, String> {
        if self.regions.iter().any(|r| r.0 == name) {
            return Ok(false);
        }
        self.regions.push((name.into(), String::new(), vec![]));
        Ok(true)
    }
    fn region_set_type(&mut self, region: &str, kind: &str) -> Result<(), String> {
        self.regions.iter_mut().find(|r| r.0 == region).unwrap().1 = kind.into();
        Ok(())
    }
    fn region_add_boundary(&mut self, region: &str, area: [i32; 4]) -> Result<bool, String> {
        self.regions.iter_mut().find(|r| r.0 == region).unwrap().2.push(area);
        Ok(true)
    }
    fn group_create(&mut self, region: &str, name: &str) -> Result<bool, String> {
        if self.group(name).is_some() {
            return Ok(false);
        }
        self.groups.push((name.into(), region.into(), "PHYSICAL_CLUSTER".into(), vec![]));
        Ok(true)
    }
    fn group_set_type(&mut self, group: &str, kind: &str) -> Result<(), String> {
        self.groups.iter_mut().find(|g| g.0 == group).unwrap().2 = kind.into();
        Ok(())
    }
    /// odb: out of any group the instance is in, then PREPENDED to this one.
    fn group_add_inst(&mut self, group: &str, inst: &str) -> Result<(), String> {
        for g in &mut self.groups {
            g.3.retain(|i| i != inst);
        }
        self.groups.iter_mut().find(|g| g.0 == group).unwrap().3.insert(0, inst.into());
        Ok(())
    }
    fn inst_create(&mut self, master: &str, name: &str) -> Result<bool, String> {
        if self.inst_masters.iter().any(|(i, _)| i == name) {
            return Ok(false);
        }
        self.inst_masters.push((name.into(), master.into()));
        Ok(true)
    }
    /// odb: nothing when already on `net`; off any other net first; PREPENDED to the net's list.
    fn connect(&mut self, inst: &str, mterm: &str, net: &str) -> Result<(), String> {
        let key = (inst.to_string(), mterm.to_string());
        if self.iterm_nets.get(&key).map(String::as_str) == Some(net) {
            return Ok(());
        }
        self.disconnect(inst, mterm)?;
        if !self.nets.iter().any(|n| n == net) {
            self.nets.push(net.into());
        }
        self.iterm_nets.insert(key.clone(), net.into());
        self.net_terms.entry(net.into()).or_default().insert(0, key);
        Ok(())
    }
    fn disconnect(&mut self, inst: &str, mterm: &str) -> Result<(), String> {
        let key = (inst.to_string(), mterm.to_string());
        if let Some(n) = self.iterm_nets.remove(&key) {
            self.net_terms.get_mut(&n).unwrap().retain(|k| *k != key);
        }
        Ok(())
    }
}

const AREA1: [i32; 4] = [30000, 30000, 650000, 490000];
const AREA2: [i32; 4] = [30000, 510000, 650000, 970000];

/// The reference's fixture: modules u_aes_1 and u_aes_2 under the top (getChildren lists the
/// newest first), one leaf in each and one in the top; PD_TOP ".", PD_AES_1, PD_AES_2.
fn aes(areas: bool) -> Mem {
    let mut m = Mem::default();
    m.children.insert(
        String::new(),
        vec![("u_aes_2".into(), "aes_cipher_top_2".into()), ("u_aes_1".into(), "aes_cipher_top".into())],
    );
    m.insts = vec![("u_aes_1/i0".into(), "u_aes_1".into()), ("u_aes_2/i0".into(), "u_aes_2".into()), ("i0".into(), ".".into())];
    m.domain("PD_TOP", ".");
    m.domain("PD_AES_1", "u_aes_1");
    m.domain("PD_AES_2", "u_aes_2");
    if areas {
        m.pd("PD_AES_1").area = Some(AREA1);
        m.pd("PD_AES_2").area = Some(AREA2);
    }
    m
}

fn eval(m: &mut Mem) -> (Result<bool, Stop>, Log) {
    let mut log = Log { trace: true, ..Default::default() };
    let r = eval_upf(m, &Liberty::default(), &mut log);
    (r, log)
}

/// One EXCLUSIVE (fence) region per non-top domain, named after it, bounded by its area; none for
/// the top domain, whose region would fence every instance.
#[test]
fn region_per_domain_with_area() {
    let mut m = aes(true);
    assert!(eval(&mut m).0.unwrap());
    assert_eq!(
        m.regions,
        vec![("PD_AES_1".into(), "EXCLUSIVE".into(), vec![AREA1]), ("PD_AES_2".into(), "EXCLUSIVE".into(), vec![AREA2])]
    );
}

/// Each domain's POWER_DOMAIN group sits in its region and holds exactly its module's instances;
/// the top domain has no group, so a top-module instance stays ungrouped.
#[test]
fn instances_grouped_by_module() {
    let mut m = aes(true);
    assert!(eval(&mut m).0.unwrap());
    assert_eq!(m.group("PD_AES_1").unwrap(), &("PD_AES_1".into(), "PD_AES_1".into(), "POWER_DOMAIN".into(), vec!["u_aes_1/i0".into()]));
    assert_eq!(m.group("PD_AES_2").unwrap().3, vec!["u_aes_2/i0".to_string()]);
    assert_eq!(m.get("PD_AES_1").group.as_deref(), Some("PD_AES_1"));
    assert_eq!(m.inst_group("i0"), None);
    assert_eq!(m.get("PD_TOP").group, None);
    assert!(m.get("PD_TOP").top);
}

/// The sub-block domains hang off the top domain; the top has no parent.
#[test]
fn domain_hierarchy() {
    let mut m = aes(true);
    let (r, log) = eval(&mut m);
    assert!(r.unwrap());
    assert_eq!(m.get("PD_AES_1").parent.as_deref(), Some("PD_TOP"));
    assert_eq!(m.get("PD_AES_2").parent.as_deref(), Some("PD_TOP"));
    assert_eq!(m.get("PD_TOP").parent, None);
    // The walk follows getChildren order (newest first), as the reference's trace shows.
    let parents: Vec<_> = log.lines.iter().filter(|l| l.contains("set_parent")).collect();
    assert_eq!(parents, ["VYGEV|set_parent|PD_AES_2|PD_TOP", "VYGEV|set_parent|PD_AES_1|PD_TOP"]);
}

/// A domain with no area still gets a region, with no boundary, and the reference warns (UPF-0021).
#[test]
fn domain_without_area_has_empty_region() {
    let mut m = aes(false);
    m.pd("PD_AES_1").area = Some(AREA1);
    let (r, log) = eval(&mut m);
    assert!(r.unwrap());
    assert_eq!(m.regions[1], ("PD_AES_2".into(), "EXCLUSIVE".into(), vec![]));
    assert_eq!(m.regions[0].2, vec![AREA1]);
    assert!(log.lines.contains(&"[WARNING UPF-0021] No area specified for PD_AES_2 power domain".to_string()));
    assert_eq!(log.warnings, 1);
}

/// Only a top domain: no region at all (the reference's test sets an area on an undeclared domain
/// first, which the reader refuses with UPF-0016 and leaves nothing behind).
#[test]
fn only_a_top_domain_leaves_no_region() {
    let mut m = Mem::default();
    m.domain("PD_TOP", ".");
    assert!(eval(&mut m).0.unwrap());
    assert!(m.regions.is_empty() && m.groups.is_empty());
}

/// Longest PLAIN-STRING prefix: `u_aes_10` falls in PD_AES_1 (element `u_aes_1`); a module below a
/// domain's module matches that same domain, which the walk then makes its OWN parent (last write
/// wins); a module no element prefixes belongs to the top domain and sets nothing. Measured on the
/// reference (variant nested_prefix).
#[test]
fn prefix_match_and_self_parent() {
    let mut m = Mem::default();
    m.children.insert(
        String::new(),
        vec![
            ("u_aes_2".into(), "m2".into()),
            ("u_misc".into(), "mmisc".into()),
            ("u_aes_10".into(), "m10".into()),
            ("u_aes_1".into(), "m1".into()),
        ],
    );
    m.children.insert("m1".into(), vec![("u_aes_1/sub".into(), "msub".into())]);
    m.insts = ["u_aes_1", "u_aes_1/sub", "u_aes_10", "u_misc", "u_aes_2", "."]
        .iter()
        .map(|p| (if *p == "." { "i0".to_string() } else { format!("{p}/i0") }, p.to_string()))
        .collect();
    m.domain("PD_TOP", ".");
    m.domain("PD_AES_1", "u_aes_1");
    m.domain("PD_AES_2", "u_aes_2");
    let (r, log) = eval(&mut m);
    assert!(r.unwrap());
    let parents: Vec<_> = log.lines.iter().filter(|l| l.contains("set_parent")).map(String::as_str).collect();
    assert_eq!(
        parents,
        [
            "VYGEV|set_parent|PD_AES_2|PD_TOP",
            "VYGEV|set_parent|PD_AES_1|PD_TOP",
            "VYGEV|set_parent|PD_AES_1|PD_TOP",
            "VYGEV|set_parent|PD_AES_1|PD_AES_1"
        ]
    );
    assert_eq!(m.get("PD_AES_1").parent.as_deref(), Some("PD_AES_1"));
    // Prepended: the group lists its instances newest first.
    assert_eq!(m.group("PD_AES_1").unwrap().3, ["u_aes_10/i0", "u_aes_1/sub/i0", "u_aes_1/i0"]);
    assert_eq!(m.inst_group("u_misc/i0"), None);
}

/// Logic ports: a net, then a terminal, each. The reference's `success` is STICKY: once one port
/// fails, later ports still get a net but no terminal.
#[test]
fn logic_port_failure_is_sticky() {
    let mut m = Mem { ports: vec!["a".into(), "b".into(), "c".into()], nets: vec!["b".into()], ..Default::default() };
    let (r, log) = eval(&mut m);
    assert!(r.unwrap());
    assert_eq!(m.nets, ["b", "a", "c"]);
    assert_eq!(m.bterms, ["a"]);
    assert_eq!(
        log.lines,
        [
            "VYGEV|net_create|a|1",
            "VYGEV|bterm_create|a|a|1",
            "VYGEV|net_create|b|0",
            "[WARNING UPF-0023] Creation of 'b' dbNet from UPF Logic Port failed",
            "VYGEV|net_create|c|1",
        ]
    );
}

/// Two domains naming the same path is an error the reference stops on (UPF-0019).
#[test]
fn a_path_in_two_domains_is_an_error() {
    let mut m = aes(true);
    m.domain("PD_DUP", "u_aes_1");
    match eval(&mut m).0 {
        Err(Stop::Error { tool: "UPF", id: 19, msg }) => assert_eq!(msg, "multiple power domain definitions for the same path u_aes_1"),
        r => panic!("{r:?}"),
    }
}

/// No domain carries "." : UPF-0029, after the regions and groups are made.
#[test]
fn no_top_domain_is_an_error() {
    let mut m = Mem::default();
    m.domain("PD_A", "u_a");
    assert!(matches!(eval(&mut m).0, Err(Stop::Error { tool: "UPF", id: 29, .. })));
    assert_eq!(m.regions.len(), 1);
}

/// A region that already exists (a second floorplan) stops the step with UPF-0020 and a false
/// return, which the floorplan ignores; nothing after it runs.
#[test]
fn a_taken_region_name_stops_with_false() {
    let mut m = aes(true);
    m.regions.push(("PD_AES_1".into(), "INCLUSIVE".into(), vec![]));
    let (r, log) = eval(&mut m);
    assert!(!r.unwrap());
    assert!(log.lines.contains(&"[WARNING UPF-0020] Creation of PD_AES_1 region failed".to_string()));
    assert!(m.groups.is_empty() && m.get("PD_AES_1").parent.is_none());
}


const LIB: &str = r#"library (t) { time_unit : "1ns"; capacitive_load_unit (1, pf);
  cell (inv_2) { area : 5.0; pin (A) { direction : input; } pin (Y) { direction : output; function : "!A"; } }
  cell (inv_1) { area : 3.75; pin (A) { direction : input; } pin (Y) { direction : output; function : "!A"; } }
  cell (inv_1b) { area : 3.75; pin (A) { direction : input; } pin (Y) { direction : output; function : "!A"; } }
  cell (buf) { area : 3.0; pin (A) { direction : input; } pin (X) { direction : output; function : "A"; } }
  cell (iso_and) { area : 10.0; is_isolation_cell : true;
    pin (A) { direction : input; isolation_cell_data_pin : true; }
    pin (SLEEP_B) { direction : input; isolation_cell_enable_pin : true; }
    pin (X) { direction : output; function : "(A&SLEEP_B)"; } }
  cell (iso_or) { area : 20.0; is_isolation_cell : true;
    pin (A) { direction : input; isolation_cell_data_pin : true; }
    pin (SLEEP) { direction : input; isolation_cell_enable_pin : true; }
    pin (X) { direction : output; function : "(A|SLEEP)"; } }
  cell (and2) { area : 6.0; pin (A) { direction : input; } pin (B) { direction : input; }
    pin (X) { direction : output; function : "(A&B)"; } } }"#;

fn liberty() -> Liberty {
    Liberty(vec![vyges_sta::liberty::Library::read(&vyges_sta::liberty_parse::parse(LIB).unwrap()).unwrap()])
}

fn io(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// sky130-like masters in library order; an inverter's DBU² area is 1,000,000.
fn netlist_masters() -> Vec<MasterRow> {
    let inv = io(&[("A", "INPUT"), ("Y", "OUTPUT")]);
    vec![
        ("inv_2".into(), inv.clone(), 1_200_000),
        ("inv_1".into(), inv.clone(), 1_000_000),
        ("inv_1b".into(), inv, 1_000_000),
        ("buf".into(), io(&[("A", "INPUT"), ("X", "OUTPUT")]), 900_000),
        ("iso_and".into(), io(&[("A", "INPUT"), ("SLEEP_B", "INPUT"), ("X", "OUTPUT")]), 2_000_000),
        ("iso_or".into(), io(&[("A", "INPUT"), ("SLEEP", "INPUT"), ("X", "OUTPUT")]), 3_000_000),
        ("and2".into(), io(&[("A", "INPUT"), ("B", "INPUT"), ("X", "OUTPUT")]), 1_500_000),
    ]
}

/// The reference's isolation regression in miniature: PD_D1 (`d1`) drives two pins in PD_D3 (`d3`)
/// from `d1/_2_/X` on net d1_out2; one isolation strategy on PD_D1 at location `parent`.
fn iso_design(sense: &str, cells: &[&str]) -> Mem {
    let mut m = Mem { masters: netlist_masters(), ports: vec!["d1_iso_control".into()], ..Default::default() };
    m.children.insert(String::new(), vec![("d3".into(), "m3".into()), ("d1".into(), "m1".into())]);
    m.domain("PD_TOP", ".");
    m.domain("PD_D1", "d1");
    m.domain("PD_D3", "d3");
    m.leaf("d1/_2_", "buf", "d1", &[("X", "d1_out2")]);
    m.leaf("d3/_0_", "buf", "d3", &[("A", "d1_out2")]);
    m.leaf("d3/_2_", "buf", "d3", &[("A", "d1_out2")]);
    m.isolations_of.insert("PD_D1".into(), vec!["iso_d_1".into()]);
    m.isolations.insert(
        "iso_d_1".into(),
        IsolationRule {
            applies_to: "both".into(),
            clamp_value: "0".into(),
            isolation_signal: "d1_iso_control".into(),
            isolation_sense: sense.into(),
            location: "parent".into(),
            cells: cells.iter().map(|c| c.to_string()).collect(),
        },
    );
    m
}

fn eval_lib(m: &mut Mem) -> (Result<bool, Stop>, Log) {
    let mut log = Log { trace: true, ..Default::default() };
    let r = eval_upf(m, &liberty(), &mut log);
    (r, log)
}

/// What the insertion step logged: everything after the grouping's last write.
fn after_grouping(log: &Log) -> Vec<&str> {
    let first = log.lines.iter().rposition(|l| l.starts_with("VYGEV|add_inst|PD_D3|d3/")).map_or(0, |i| i + 1);
    log.lines[first..].iter().map(String::as_str).collect()
}

/// The reference's trace for `d1/_2_` (upf `isolation` regression): one `<net>_o` net, the cell
/// `<inst>_<net>_<net>_o_isolation` wired output, enable, data in that order, then each foreign
/// pin, in the NET'S order (newest connection first), moved onto the new net. Location `parent`
/// with the top as parent: no group, so the cell is fenced nowhere.
#[test]
fn isolation_cell_between_domains_as_the_reference_wires_it() {
    let mut m = iso_design("low", &["iso_and"]);
    let (r, log) = eval_lib(&mut m);
    assert!(r.unwrap());
    assert_eq!(
        after_grouping(&log),
        [
            "VYGEV|net_create|d1_out2_o|1",
            "VYGEV|inst_create|d1/_2__d1_out2_d1_out2_o_isolation|iso_and|1",
            "VYGEV|connect|d1/_2__d1_out2_d1_out2_o_isolation/X|d1_out2_o",
            "VYGEV|connect|d1/_2__d1_out2_d1_out2_o_isolation/SLEEP_B|d1_iso_control",
            "VYGEV|connect|d1/_2__d1_out2_d1_out2_o_isolation/A|d1_out2",
            "VYGEV|disconnect|d3/_2_/A|d1_out2",
            "VYGEV|connect|d3/_2_/A|d1_out2_o",
            "VYGEV|disconnect|d3/_0_/A|d1_out2",
            "VYGEV|connect|d3/_0_/A|d1_out2_o",
        ]
    );
    assert_eq!(m.inst_group("d1/_2__d1_out2_d1_out2_o_isolation"), None);
}

/// Sense `high` on an AND cell whose enable is plain: the control needs an inverter — the
/// SMALLEST-area inverter in library order (inv_1, not inv_2; inv_1b ties it and comes later),
/// named `<cell>_inv_control` with a net of the same name (the reference's d2 trace).
#[test]
fn an_inverted_control_takes_the_first_smallest_inverter() {
    let mut m = iso_design("high", &["iso_and"]);
    let (r, log) = eval_lib(&mut m);
    assert!(r.unwrap());
    let c = "d1/_2__d1_out2_d1_out2_o_isolation";
    assert_eq!(
        after_grouping(&log)[..8],
        [
            "VYGEV|net_create|d1_out2_o|1".to_string(),
            format!("VYGEV|inst_create|{c}|iso_and|1"),
            format!("VYGEV|connect|{c}/X|d1_out2_o"),
            format!("VYGEV|inst_create|{c}_inv_control|inv_1|1"),
            format!("VYGEV|connect|{c}_inv_control/A|d1_iso_control"),
            format!("VYGEV|net_create|{c}_inv_control|1"),
            format!("VYGEV|connect|{c}_inv_control/Y|{c}_inv_control"),
            format!("VYGEV|connect|{c}/SLEEP_B|{c}_inv_control"),
        ]
    );
}

/// The cell choice adds the inverter MASTER's `getArea` (DBU², here 1,000,000) to a Liberty area
/// (µm²): with sense high and clamp 1, iso_and (10) needs two inverters and loses to iso_or (20),
/// which needs none. On Liberty areas alone (10 + 2 × 3.75) iso_and would win.
#[test]
fn the_cell_choice_adds_the_inverters_dbu_area() {
    let mut m = iso_design("high", &["iso_and", "iso_or"]);
    m.isolations.get_mut("iso_d_1").unwrap().clamp_value = "1".into();
    // iso_and, sense high clamp 1: AND ⇒ (control = high = invert, output = clamp = invert): +2 inverters.
    // iso_or, sense high clamp 1: OR ⇒ (control = !high = keep, output = !clamp = keep): 20.
    let (r, log) = eval_lib(&mut m);
    assert!(r.unwrap());
    assert!(log.lines.contains(&"VYGEV|inst_create|d1/_2__d1_out2_d1_out2_o_isolation|iso_or|1".to_string()), "{:#?}", log.lines);
}

/// check_isolation_match: AND clamps to 0 with the enable active-high (sense high inverts it
/// unless the enable operand is itself a NOT); OR clamps to 1; only the enable operand's OWN top
/// node counts as an inversion; any other top operator is "unknown" (UPF-0018); a function not
/// reading the enable is UPF-0017.
#[test]
fn isolation_match_rules() {
    let f = |s: &str| FuncExpr::parse(s).unwrap();
    let mut log = Log::default();
    assert_eq!(check_isolation_match(&f("(A&E)"), "E", false, false, &mut log), Some((false, false)));
    assert_eq!(check_isolation_match(&f("(A&E)"), "E", true, true, &mut log), Some((true, true)));
    assert_eq!(check_isolation_match(&f("(A&!E)"), "E", true, false, &mut log), Some((false, false)));
    assert_eq!(check_isolation_match(&f("(A|E)"), "E", false, true, &mut log), Some((true, false)));
    assert_eq!(check_isolation_match(&f("(!E|A)"), "E", false, false, &mut log), Some((false, true)));
    // The enable two levels down: its operand's top node is an AND, not a NOT.
    assert_eq!(check_isolation_match(&f("(A|(B&!E))"), "E", false, false, &mut log), Some((true, true)));
    assert_eq!(log.warnings, 0);
    assert_eq!(check_isolation_match(&f("(A^E)"), "E", false, false, &mut log), None);
    assert_eq!(check_isolation_match(&f("(A&B)"), "E", false, false, &mut log), None);
    assert_eq!(log.lines, ["[WARNING UPF-0018] unknown isolation cell function", "[WARNING UPF-0017] isolation cell has no enable port"]);
}

/// An isolation with no cells warns UPF-0024 — once per foreign CONNECTION, as the cell search
/// runs per connection — and inserts nothing (gpl region01's one line after the grouping);
/// `applies_to inputs` skips an output source without a word.
#[test]
fn an_isolation_with_no_cells_or_not_applying_inserts_nothing() {
    let mut m = iso_design("low", &[]);
    let (r, log) = eval_lib(&mut m);
    assert!(r.unwrap());
    assert_eq!(after_grouping(&log), ["[WARNING UPF-0024] Isolation iso_d_1 defined, but no cells defined."; 2]);

    let mut m = iso_design("low", &["iso_and"]);
    m.isolations.get_mut("iso_d_1").unwrap().applies_to = "inputs".into();
    let (r, log) = eval_lib(&mut m);
    assert!(r.unwrap());
    assert!(!log.lines.iter().any(|l| l.contains("isolation|")), "{:#?}", log.lines);
}

fn rule(elements: &[&str], excluded: &[&str], force: bool, no_shift: bool) -> ShiftRule {
    ShiftRule {
        elements: elements.iter().map(|e| e.to_string()).collect(),
        exclude_elements: excluded.iter().map(|e| e.to_string()).collect(),
        force_shift: force,
        no_shift,
        rule: "both".into(),
        applies_to: "both".into(),
        location: "self".into(),
        cell_name: "and2".into(),
        cell_input: "A".into(),
        cell_output: "X".into(),
        ..Default::default()
    }
}

/// find_shift_strategy: no elements lists everything; "." lists everything; a force_shift that
/// lists and does not exclude returns at once; a listing no_shift vetoes all but that; otherwise
/// the LAST listing, non-excluding, shifting strategy.
#[test]
fn shift_strategy_selection() {
    let t = ITerm { inst: "u1".into(), mterm: "X".into() };
    let pick = |rules: &[(&str, ShiftRule)]| {
        let mut m = Mem::default();
        m.shifters_of.insert("PD".into(), rules.iter().map(|(n, _)| n.to_string()).collect());
        for (n, r) in rules {
            m.shifters.insert(n.to_string(), r.clone());
        }
        find_shift_strategy(&m, "PD", &t).unwrap()
    };
    assert_eq!(pick(&[("a", rule(&[], &[], false, false)), ("b", rule(&["."], &[], false, false))]), Some("b".into()));
    assert_eq!(pick(&[("a", rule(&["u1"], &[], false, false)), ("b", rule(&["u2"], &[], false, false))]), Some("a".into()));
    assert_eq!(pick(&[("a", rule(&[], &["u1"], false, false))]), None);
    assert_eq!(pick(&[("a", rule(&[], &[], false, true)), ("b", rule(&[], &[], false, false))]), None);
    assert_eq!(pick(&[("a", rule(&[], &[], false, true)), ("b", rule(&[], &[], true, false))]), Some("b".into()));
    assert_eq!(pick(&[("a", rule(&[], &[], true, false)), ("b", rule(&[], &[], false, false))]), Some("a".into()));
}

/// validate_shifting_strategy: both voltages needed (UPF-0053); the direction is seen from the
/// SOURCE terminal, so an input source reverses it; the threshold is `difference < threshold`
/// (equal passes); applies_to by the source's direction.
#[test]
fn shifting_validation_rules() {
    let mut m = Mem { masters: netlist_masters(), ..Default::default() };
    m.leaf("o", "buf", "", &[]);
    let out = ITerm { inst: "o".into(), mterm: "X".into() };
    let inp = ITerm { inst: "o".into(), mterm: "A".into() };
    m.voltage.insert("LO".into(), 1.0);
    m.voltage.insert("HI".into(), 1.5);
    let mut log = Log::default();
    let mut r = rule(&[], &[], false, false);
    r.rule = "low_to_high".into();
    assert!(validate_shifting_strategy(&m, &mut log, &r, &out, "LO", "HI").unwrap());
    assert!(!validate_shifting_strategy(&m, &mut log, &r, &inp, "LO", "HI").unwrap(), "an input reverses it");
    assert!(validate_shifting_strategy(&m, &mut log, &r, &inp, "HI", "LO").unwrap());
    r.threshold = 0.5;
    assert!(validate_shifting_strategy(&m, &mut log, &r, &out, "LO", "HI").unwrap(), "equal to the threshold passes");
    r.threshold = 0.6;
    assert!(!validate_shifting_strategy(&m, &mut log, &r, &out, "LO", "HI").unwrap());
    r.threshold = 0.0;
    r.applies_to = "inputs".into();
    assert!(!validate_shifting_strategy(&m, &mut log, &r, &out, "LO", "HI").unwrap());
    assert_eq!(log.warnings, 0);
    assert!(!validate_shifting_strategy(&m, &mut log, &r, &out, "LO", "NONE").unwrap());
    assert_eq!(log.lines, ["[WARNING UPF-0053] Missing voltage information for one of the domains LO NONE"]);
}

/// Domains at different voltages: a level shifter `<iterm>_<target>_level_shifter` (names joined by
/// `_`) in the source domain's group (`self`), its input on the source net, its output on
/// `<name>_out_net`, which the TARGET moves to for an output source.
#[test]
fn level_shifter_on_an_output_moves_the_target() {
    let mut m = iso_design("low", &["iso_and"]);
    m.isolations_of.clear();
    m.voltage.insert("PD_D1".into(), 1.0);
    m.voltage.insert("PD_D3".into(), 1.8);
    m.shifters_of.insert("PD_D1".into(), vec!["ls_1".into()]);
    m.shifters.insert("ls_1".into(), rule(&["."], &[], false, false));
    let (r, log) = eval_lib(&mut m);
    assert!(r.unwrap());
    let ls = "d1/_2__X_d3/_2__A_level_shifter";
    let tail: Vec<_> = log.lines.iter().skip_while(|l| !l.contains("inst_create")).take(7).cloned().collect();
    assert_eq!(
        tail,
        [
            format!("VYGEV|inst_create|{ls}|and2|1"),
            format!("VYGEV|add_inst|PD_D1|{ls}"),
            format!("VYGEV|net_create|{ls}_out_net|1"),
            format!("VYGEV|connect|{ls}/A|d1_out2"),
            format!("VYGEV|connect|{ls}/X|{ls}_out_net"),
            "VYGEV|disconnect|d3/_2_/A|d1_out2".to_string(),
            format!("VYGEV|connect|d3/_2_/A|{ls}_out_net"),
        ]
    );
}

/// A level shifter whose cell is empty is refused: the cell is not stored in the database, so one
/// read from a database has lost it (the reference, in the same run as its read, has not).
#[test]
fn a_level_shifter_without_its_cell_is_refused() {
    let mut m = iso_design("low", &["iso_and"]);
    let mut r = rule(&["."], &[], false, false);
    r.cell_name.clear();
    m.shifters_of.insert("PD_D1".into(), vec!["ls_1".into()]);
    m.shifters.insert("ls_1".into(), r);
    assert!(matches!(eval_lib(&mut m).0, Err(Stop::Refused(_))));
}
