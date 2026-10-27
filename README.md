# vyges-upf

Power intent (UPF) for designs with several power domains: domains, isolation, level shifters, power switches.

> **Status: in development.** `run`, `write` and `eval` are built. A UPF file is Tcl: a variable or command
> substitution, or a Tcl command outside the supported UPF set, is refused (exit 3), not guessed at.

## Why this exists

Analog and mixed-signal chips run several supplies. The power intent says which logic sits in which domain and what guards each crossing; floorplanning, power-grid generation and timing all need it.

## Usage

```
vyges physical upf run   <design.odb> <intent.upf> -o OUT.odb
vyges physical upf write <design.odb> [-o FILE]
vyges physical upf eval  <design.odb> -o OUT.odb [--liberty FILE]...
vyges physical upf --describe
```

- `run`: the power intent read into the database: power domains, logic ports, power switches,
  isolation and level-shifter strategies, domain areas and voltages.
- `write`: the database's power intent written back as UPF.
- `eval`: the intent made physical: a fence region and group per domain, the domain hierarchy, and
  isolation cells and level shifters inserted between domains, chosen from the Liberty cells.

## Install

```
vyges install physical
```

It is part of the `vyges-physical` binary; `vyges physical upf --help` prints the full option list.

## License

Apache-2.0. See `LICENSE` and `NOTICE`.
