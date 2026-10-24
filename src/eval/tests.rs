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

#[derive(Default)]
struct Mem {
    ports: Vec<String>,
    nets: Vec<String>,
    bterms: Vec<String>,
    pds: Vec<Pd>,
    strategies: usize,
    /// module ("" = top) -> children in getChildren order: (hierarchical name, master module)
    children: BTreeMap<String, Vec<(String, String)>>,
    /// instances in block order with their module path
    insts: Vec<(String, String)>,
    /// region -> (type, boundaries)
    regions: Vec<(String, String, Vec<[i32; 4]>)>,
    /// group -> (region, type, instances in the group's own order)
    groups: Vec<(String, String, String, Vec<String>)>,
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
    fn insertion_strategies(&self) -> Result<usize, String> {
        Ok(self.strategies)
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
    let r = eval_upf(m, &mut log);
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

/// Cell insertion is the next step and is not implemented: a design with a strategy for it is
/// refused after the grouping, not evaluated in part.
#[test]
fn an_insertion_strategy_is_refused() {
    let mut m = aes(true);
    m.strategies = 1;
    assert!(matches!(eval(&mut m).0, Err(Stop::Refused(_))));
}
