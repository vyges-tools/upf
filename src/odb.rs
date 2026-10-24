// SPDX-License-Identifier: Apache-2.0
//! The power intent read from a design database, every table in the database's own order.

use crate::{Domain, Isolation, LevelShifter, LogicPort, PowerIntent, PowerSwitch};
use vyges_opendb::{Db, Result};

/// Each table read as the database iterates it: creation order, which is the order the writer
/// must keep.
pub fn read_power_intent(db: &Db) -> Result<PowerIntent> {
    Ok(PowerIntent {
        domains: read_domains(db)?,
        ports: read_ports(db)?,
        switches: read_switches(db)?,
        isolations: read_isolations(db),
        level_shifters: read_level_shifters(db)?,
    })
}

fn read_domains(db: &Db) -> Result<Vec<Domain>> {
    db.block_get_power_domains()
        .into_iter()
        .map(|name| {
            Ok(Domain { top: db.pwr_domain_is_top(&name), elements: db.pwr_domain_elements(&name)?, name })
        })
        .collect()
}

fn read_ports(db: &Db) -> Result<Vec<LogicPort>> {
    db.block_get_logic_ports()
        .into_iter()
        .map(|name| Ok(LogicPort { direction: db.logic_port_direction(&name)?, name }))
        .collect()
}

fn pairs(v: Vec<(String, String, String)>) -> Vec<(String, String)> {
    v.into_iter().map(|(a, b, _)| (a, b)).collect()
}

fn read_switches(db: &Db) -> Result<Vec<PowerSwitch>> {
    db.block_get_power_switches()
        .into_iter()
        .map(|name| {
            let out = pairs(db.pwr_switch_records(&name, "output")?);
            let cell = db.pwr_switch_get_lib_cell(&name);
            Ok(PowerSwitch {
                output: out.into_iter().next().unwrap_or_default(),
                inputs: pairs(db.pwr_switch_records(&name, "input")?),
                controls: pairs(db.pwr_switch_records(&name, "control")?),
                on_states: db.pwr_switch_records(&name, "on_state")?,
                acks: db.pwr_switch_records(&name, "ack")?,
                domain: db.pwr_switch_get_power_domain(&name),
                lib_cell: (!cell.is_empty()).then_some(cell),
                port_map: pairs(db.pwr_switch_records(&name, "port_map")?),
                name,
            })
        })
        .collect()
}

fn read_isolations(db: &Db) -> Vec<Isolation> {
    db.block_get_isolations()
        .into_iter()
        .map(|name| Isolation {
            domain: db.isolation_get_power_domain(&name),
            applies_to: db.isolation_get_applies_to(&name),
            location: db.isolation_get_location(&name),
            clamp_value: db.isolation_get_clamp_value(&name),
            isolation_signal: db.isolation_get_isolation_signal(&name),
            isolation_sense: db.isolation_get_isolation_sense(&name),
            name,
        })
        .collect()
}

fn read_level_shifters(db: &Db) -> Result<Vec<LevelShifter>> {
    db.block_get_level_shifters()
        .into_iter()
        .map(|name| {
            Ok(LevelShifter {
                domain: db.level_shifter_get_domain(&name),
                elements: db.level_shifter_list(&name, "elements")?,
                exclude_elements: db.level_shifter_list(&name, "exclude_elements")?,
                source: db.level_shifter_get_source(&name),
                sink: db.level_shifter_get_sink(&name),
                use_functional_equivalence: db.level_shifter_is_use_functional_equivalence(&name),
                applies_to: db.level_shifter_get_applies_to(&name),
                applies_to_boundary: db.level_shifter_get_applies_to_boundary(&name),
                rule: db.level_shifter_get_rule(&name),
                threshold: db.level_shifter_get_threshold(&name),
                no_shift: db.level_shifter_is_no_shift(&name),
                force_shift: db.level_shifter_is_force_shift(&name),
                location: db.level_shifter_get_location(&name),
                input_supply: db.level_shifter_get_input_supply(&name),
                output_supply: db.level_shifter_get_output_supply(&name),
                internal_supply: db.level_shifter_get_internal_supply(&name),
                name_prefix: db.level_shifter_get_name_prefix(&name),
                name_suffix: db.level_shifter_get_name_suffix(&name),
                instances: db.level_shifter_instances(&name)?,
                name,
            })
        })
        .collect()
}

/// The reader's database calls, one binding call each.
impl crate::read::UpfStore for Db {
    /// ⚠️ The binding reports the top block's name, empty without one; it cannot tell "no chip"
    /// from "a chip with no block", so both read as no chip (UPF-0033). A database written by
    /// `write_db` always has both.
    fn has_chip(&self) -> bool {
        !self.block_name().is_empty()
    }
    fn has_block(&self) -> bool {
        !self.block_name().is_empty()
    }
    fn find(&self, kind: &str, name: &str) -> std::result::Result<bool, String> {
        e(self.upf_find(kind, name))
    }
    fn dbu_per_micron(&self) -> std::result::Result<i32, String> {
        Ok(Db::dbu_per_micron(self))
    }
    fn pwr_domain_create(&mut self, name: &str) -> std::result::Result<bool, String> {
        e(Db::pwr_domain_create(self, name))
    }
    fn pwr_domain_add_element(&mut self, pd: &str, element: &str) -> std::result::Result<(), String> {
        e(Db::pwr_domain_add_element(self, pd, element))
    }
    fn pwr_domain_add_power_switch(&mut self, pd: &str, sw: &str) -> std::result::Result<(), String> {
        e(Db::pwr_domain_add_power_switch(self, pd, sw))
    }
    fn pwr_domain_add_isolation(&mut self, pd: &str, iso: &str) -> std::result::Result<(), String> {
        e(Db::pwr_domain_add_isolation(self, pd, iso))
    }
    fn pwr_domain_set_area(&mut self, pd: &str, x1: i32, y1: i32, x2: i32, y2: i32) -> std::result::Result<(), String> {
        e(Db::pwr_domain_set_area(self, pd, x1, y1, x2, y2))
    }
    fn pwr_domain_set_voltage(&mut self, pd: &str, v: f32) -> std::result::Result<(), String> {
        e(Db::pwr_domain_set_voltage(self, pd, v))
    }
    fn logic_port_create(&mut self, name: &str, direction: &str) -> std::result::Result<bool, String> {
        e(Db::logic_port_create(self, name, direction))
    }
    fn pwr_switch_create(&mut self, name: &str) -> std::result::Result<bool, String> {
        e(Db::pwr_switch_create(self, name))
    }
    fn pwr_switch_set_power_domain(&mut self, sw: &str, pd: &str) -> std::result::Result<(), String> {
        e(Db::pwr_switch_set_power_domain(self, sw, pd))
    }
    fn pwr_switch_set_out_supply_port(&mut self, sw: &str, port: &str, net: &str) -> std::result::Result<(), String> {
        e(Db::pwr_switch_set_out_supply_port(self, sw, port, net))
    }
    fn pwr_switch_add_in_supply_port(&mut self, sw: &str, port: &str, net: &str) -> std::result::Result<(), String> {
        e(Db::pwr_switch_add_in_supply_port(self, sw, port, net))
    }
    fn pwr_switch_add_control_port(&mut self, sw: &str, port: &str, net: &str) -> std::result::Result<(), String> {
        e(Db::pwr_switch_add_control_port(self, sw, port, net))
    }
    fn pwr_switch_add_acknowledge_port(&mut self, sw: &str, port: &str, net: &str, expr: &str) -> std::result::Result<(), String> {
        e(Db::pwr_switch_add_acknowledge_port(self, sw, port, net, expr))
    }
    fn pwr_switch_add_on_state(&mut self, sw: &str, state: &str, port: &str, expr: &str) -> std::result::Result<(), String> {
        e(Db::pwr_switch_add_on_state(self, sw, state, port, expr))
    }
    fn pwr_switch_set_lib_cell(&mut self, sw: &str, master: &str) -> std::result::Result<(), String> {
        e(Db::pwr_switch_set_lib_cell(self, sw, master))
    }
    fn pwr_switch_add_port_map(&mut self, sw: &str, model_port: &str, switch_port: &str) -> std::result::Result<(), String> {
        e(Db::pwr_switch_add_port_map(self, sw, model_port, switch_port))
    }
    fn isolation_create(&mut self, name: &str) -> std::result::Result<bool, String> {
        e(Db::isolation_create(self, name))
    }
    fn isolation_set_power_domain(&mut self, iso: &str, pd: &str) -> std::result::Result<(), String> {
        e(Db::isolation_set_power_domain(self, iso, pd))
    }
    fn isolation_set(&mut self, iso: &str, field: &str, value: &str) -> std::result::Result<(), String> {
        e(match field {
            "applies_to" => self.isolation_set_applies_to(iso, value),
            "clamp_value" => self.isolation_set_clamp_value(iso, value),
            "isolation_signal" => self.isolation_set_isolation_signal(iso, value),
            "isolation_sense" => self.isolation_set_isolation_sense(iso, value),
            "location" => self.isolation_set_location(iso, value),
            f => return Err(format!("unknown isolation field {f}")),
        })
    }
    fn isolation_add_isolation_cell(&mut self, iso: &str, cell: &str) -> std::result::Result<(), String> {
        e(Db::isolation_add_isolation_cell(self, iso, cell))
    }
    fn level_shifter_create(&mut self, name: &str, pd: &str) -> std::result::Result<bool, String> {
        e(Db::level_shifter_create(self, name, pd))
    }
    fn level_shifter_set(&mut self, ls: &str, field: &str, value: &str) -> std::result::Result<(), String> {
        e(match field {
            "source" => self.level_shifter_set_source(ls, value),
            "sink" => self.level_shifter_set_sink(ls, value),
            "applies_to" => self.level_shifter_set_applies_to(ls, value),
            "applies_to_boundary" => self.level_shifter_set_applies_to_boundary(ls, value),
            "rule" => self.level_shifter_set_rule(ls, value),
            "location" => self.level_shifter_set_location(ls, value),
            "input_supply" => self.level_shifter_set_input_supply(ls, value),
            "output_supply" => self.level_shifter_set_output_supply(ls, value),
            "internal_supply" => self.level_shifter_set_internal_supply(ls, value),
            "name_prefix" => self.level_shifter_set_name_prefix(ls, value),
            "name_suffix" => self.level_shifter_set_name_suffix(ls, value),
            "cell_name" => self.level_shifter_set_cell_name(ls, value),
            "cell_input" => self.level_shifter_set_cell_input(ls, value),
            "cell_output" => self.level_shifter_set_cell_output(ls, value),
            f => return Err(format!("unknown level shifter field {f}")),
        })
    }
    fn level_shifter_set_threshold(&mut self, ls: &str, v: f32) -> std::result::Result<(), String> {
        e(Db::level_shifter_set_threshold(self, ls, v))
    }
    fn level_shifter_set_bool(&mut self, ls: &str, field: &str, v: bool) -> std::result::Result<(), String> {
        e(match field {
            "no_shift" => self.level_shifter_set_no_shift(ls, v),
            "force_shift" => self.level_shifter_set_force_shift(ls, v),
            "use_functional_equivalence" => self.level_shifter_set_use_functional_equivalence(ls, v),
            f => return Err(format!("unknown level shifter flag {f}")),
        })
    }
    fn level_shifter_add_element(&mut self, ls: &str, element: &str) -> std::result::Result<(), String> {
        e(Db::level_shifter_add_element(self, ls, element))
    }
    fn level_shifter_add_exclude_element(&mut self, ls: &str, element: &str) -> std::result::Result<(), String> {
        e(Db::level_shifter_add_exclude_element(self, ls, element))
    }
    fn level_shifter_add_instance(&mut self, ls: &str, inst: &str, port: &str) -> std::result::Result<(), String> {
        e(Db::level_shifter_add_instance(self, ls, inst, port))
    }
}

fn e<T>(r: Result<T>) -> std::result::Result<T, String> {
    r.map_err(|x| x.to_string())
}
