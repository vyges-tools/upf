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
