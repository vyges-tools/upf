// SPDX-License-Identifier: Apache-2.0
//! `eval`: the power intent in the database made physical — a fence region and a group per power
//! domain, the domain hierarchy, each instance in its domain's group. The floorplan runs this
//! before it builds rows.
//!
//! One function per step, named after the step and kept in the order the command runs them;
//! [`eval_upf`] is nothing but that sequence. Every database write goes through [`EvalStore`] and,
//! with `--trace`, prints one `VYGEV|…` line in the order made.
//!
//! ⚠️ Cell insertion (isolation cells and level shifters between domains) is the step after the
//! instances are grouped; it is not implemented yet, and a design that carries an isolation or
//! level-shifter strategy is refused there rather than evaluated in part.

use std::collections::BTreeMap;

use crate::read::Log;
use crate::tcl::Stop;

/// The database reads and writes the evaluation makes, one per call the reference makes. Each
/// `*_create` returns `false` where the database refuses (the name is taken).
pub trait EvalStore {
    /// The block's logic ports, power domains: in the database's order.
    fn logic_ports(&self) -> Result<Vec<String>, String>;
    fn power_domains(&self) -> Result<Vec<String>, String>;
    fn domain_elements(&self, pd: &str) -> Result<Vec<String>, String>;
    /// `getArea`: the area when one was set.
    fn domain_area(&self, pd: &str) -> Result<Option<[i32; 4]>, String>;
    fn domain_is_top(&self, pd: &str) -> Result<bool, String>;
    /// `getParent`, `getGroup`: `None` for a null pointer.
    fn domain_parent(&self, pd: &str) -> Result<Option<String>, String>;
    fn domain_group(&self, pd: &str) -> Result<Option<String>, String>;
    /// Isolation and level-shifter strategies in the block: what the cell-insertion step acts on.
    fn insertion_strategies(&self) -> Result<usize, String>;
    /// The children of `module` ("" = the top module) in `getChildren` order: (hierarchical name of
    /// the dbModInst, name of its master module).
    fn module_children(&self, module: &str) -> Result<Vec<(String, String)>, String>;
    /// Every instance in `getInsts` order with its module's path: "" for none, "." for the top
    /// module, else the hierarchical name of the module's dbModInst.
    fn inst_module_paths(&self) -> Result<Vec<(String, String)>, String>;

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
}

/// `path -> power domain`, ordered as the reference's `std::map` iterates it (byte order).
type PathToDomain = BTreeMap<String, String>;

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

/// Evaluate the power intent in the database. `Ok(false)` where the reference returns false (a
/// region or group could not be created; the caller carries on); an error the reference raises
/// stops the evaluation.
pub fn eval_upf(s: &mut impl EvalStore, log: &mut Log) -> Result<bool, Stop> {
    instantiate_logic_ports(s, log)?;
    if db(s.power_domains())?.is_empty() {
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
    insert_cells(s)?;
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
            db(s.group_add_inst(&group, &inst))?;
            trace(log, format!("VYGEV|add_inst|{group}|{inst}"));
        }
    }
    Ok(())
}

/// Isolation cells and level shifters on the connections that cross domains: not implemented yet.
/// A design with no strategy for either gets nothing from this step in the reference either.
fn insert_cells(s: &mut impl EvalStore) -> Result<(), Stop> {
    let n = db(s.insertion_strategies())?;
    if n > 0 {
        return Err(Stop::Refused(format!(
            "{n} isolation/level-shifter strateg{}: inserting their cells is not implemented yet",
            if n == 1 { "y" } else { "ies" }
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
