# vyges-upf

Power intent (UPF) for designs with several power domains: domains, isolation, level shifters, power switches.

> **Status: in development.** `write` is available: it writes the power intent held in a design
> database as UPF. `run` (reading UPF into the database) refuses with exit 3 until it is built.

## Why this exists

Analog and mixed-signal chips run several supplies. The power intent says which logic sits in which domain and what guards each crossing; floorplanning, power-grid generation and timing all need it.

## Usage

```
vyges physical upf run <design.odb> <intent.upf> -o OUT.odb
vyges physical upf write <design.odb>
vyges physical upf --describe
```

## Install

```
vyges install physical
```

## License

Apache-2.0. See `LICENSE` and `NOTICE`.
