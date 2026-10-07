// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ProjectFlags: u32 {
        const PRE_PROCESS_ONLY = 0b1;
        const HEAT_BALANCE = 0b10;
        const DETAILED_OUTPUT_HEATING_COOLING = 0b100;

        // Preserve the legacy bit layout so existing serialized/debugged values remain stable.
        const FHS_ASSUMPTIONS = 0b100000000;
        const FHS_FEE_ASSUMPTIONS = 0b1000000000;
        const FHS_NOT_A_ASSUMPTIONS = 0b10000000000;
        const FHS_NOT_B_ASSUMPTIONS = 0b100000000000;
        const FHS_FEE_NOT_A_ASSUMPTIONS = 0b1000000000000;
        const FHS_FEE_NOT_B_ASSUMPTIONS = 0b10000000000000;
        const FHS_COMPLIANCE = 0b100000000000000;
    }
}
