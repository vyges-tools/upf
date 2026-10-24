// SPDX-License-Identifier: Apache-2.0
//! `vyges_upf` — power intent (UPF) for designs with several power domains: domains, isolation, level shifters, power switches.
//!
//! Stages are added here one at a time, each named after the step it performs and kept in the order
//! the command runs them.
