// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

//! Minimal shared source boundary for the FHS target runner.

pub mod in_memory_output;
pub mod project_flags;
pub mod runtime;
mod target_scenario;

pub use in_memory_output::InMemoryOutputWriter;
pub use target_scenario::{
    auto_assign_cooling_systems, build_space_heat_system_from_template,
    detect_and_apply_compliance_settings, expand_event_patterns, merge_model,
    prepare_effective_scenario, prepare_target_scenario, reject_structural_change,
    update_zone_fabric,
};
