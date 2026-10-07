use crate::future_homes_standard::fhs_compliance_response::{
    CalculatedComplianceResult, FhsComplianceResponse,
};
use crate::future_homes_standard::fhs_part_f_validation::part_f::validate_dwelling_ventilation;
use crate::future_homes_standard::future_homes_standard::{calc_tfa, final_preprocessing};
use crate::future_homes_standard::future_homes_standard_notional::apply_fhs_notional_preprocessing;
use crate::future_homes_standard::input::InputForProcessing;
use crate::future_homes_standard::metrics::{energy_efficiency_rating, Metrics};
use crate::future_homes_standard::project_lookups::by_fuel;
use crate::HemWrapper;
use crate::{CalculationKey, FhsFlags};
use anyhow::anyhow;
use future_homes_standard::apply_fhs_postprocessing;
use future_homes_standard_fee::{apply_fhs_fee_postprocessing, apply_fhs_fee_preprocessing};
use home_energy_model::input::{CustomEnergySourceFactor, Input};
use home_energy_model::output::{Output, OutputCore, OutputSummary};
use home_energy_model::output_writer::OutputWriter;
use home_energy_model::{write_core_output_files, HemResponse};
use home_energy_model::{CalculationResult, OutputFormat};
use indexmap::IndexMap;
use rayon::prelude::*;
use serde::Serialize;
use serde_json::ser::PrettyFormatter;
use serde_json::Serializer;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tracing::error;

mod fhs_appliance;
mod fhs_compliance_response;
mod fhs_hw_events;
mod fhs_imev_scheduler;
mod fhs_part_f_validation;
pub mod fhs_schema_validation;
pub(crate) mod fhs_sleeved_dhn_validation;
pub(crate) mod fhs_storeys_validation;
pub(crate) mod fhs_ventilation;
pub(crate) mod fhs_window_validation;
#[allow(clippy::module_inception)]
pub mod future_homes_standard;
pub(crate) mod future_homes_standard_fee;
pub(crate) mod future_homes_standard_notional;
pub(crate) mod input;
pub(crate) mod metrics;
pub(crate) mod project_lookups;

/// A HEM wrapper for all single calculations using the FHS wrapper.
pub struct FhsIndividualCalcWrapper;

impl Default for FhsIndividualCalcWrapper {
    fn default() -> Self {
        Self::new()
    }
}

impl FhsIndividualCalcWrapper {
    pub fn new() -> Self {
        Self {}
    }
}

impl HemWrapper for FhsIndividualCalcWrapper {
    fn apply_preprocessing(
        &self,
        input: InputForProcessing,
        custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
        flags: &FhsFlags,
    ) -> anyhow::Result<HashMap<CalculationKey, InputForProcessing>> {
        let calculations: Vec<(CalculationKey, FhsFlags)> = flags
            .iter()
            .map(|flag| {
                let calculation_key: CalculationKey = (&flag).into();
                (calculation_key, flag)
            })
            .collect();

        vec![input; calculations.len()]
            .into_par_iter()
            .enumerate()
            .map(|(i, mut input)| {
                let (key, flag) = &calculations[i];
                do_fhs_preprocessing(&mut input, custom_energy_supply_factors, flag)?;
                Ok((*key, input))
            })
            .collect::<anyhow::Result<HashMap<CalculationKey, InputForProcessing>>>()
    }

    fn apply_postprocessing(
        &self,
        output: &impl OutputWriter,
        results: &HashMap<CalculationKey, CalculationResult>,
        flags: &FhsFlags,
        core_output_formats: &[OutputFormat],
        heat_balance: bool,
        detailed_output_heating_cooling: bool,
        custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
    ) -> anyhow::Result<Option<HemResponse>> {
        let calculations: Vec<(CalculationKey, FhsFlags)> = flags
            .iter()
            .map(|flag| {
                let calculation_key: CalculationKey = (&flag).into();
                (calculation_key, flag)
            })
            .collect();

        calculations
            .par_iter()
            .map(|(key, flag)| {
                do_fhs_postprocessing(
                    output,
                    &results[key],
                    flag,
                    core_output_formats,
                    heat_balance,
                    detailed_output_heating_cooling,
                    custom_energy_supply_factors,
                )?;
                Ok(())
            })
            .collect::<anyhow::Result<()>>()?;
        Ok(None)
    }
}

/// A HEM wrapper for full FHS compliance calculations.
pub struct FhsComplianceWrapper;

impl Default for FhsComplianceWrapper {
    fn default() -> Self {
        Self::new()
    }
}

impl FhsComplianceWrapper {
    pub fn new() -> Self {
        Self {}
    }
}

impl HemWrapper for FhsComplianceWrapper {
    fn apply_preprocessing(
        &self,
        input: InputForProcessing,
        custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
        _flags: &FhsFlags,
    ) -> anyhow::Result<HashMap<CalculationKey, InputForProcessing>> {
        vec![input; FHS_COMPLIANCE_CALCULATIONS.len()]
            .into_par_iter()
            .enumerate()
            .map(|(i, mut input)| {
                let (key, flag) = &FHS_COMPLIANCE_CALCULATIONS[i];
                do_fhs_preprocessing(&mut input, custom_energy_supply_factors, flag)?;
                Ok((*key, input))
            })
            .collect::<anyhow::Result<HashMap<CalculationKey, InputForProcessing>>>()
    }

    fn apply_postprocessing(
        &self,
        output: &impl OutputWriter,
        results: &HashMap<CalculationKey, CalculationResult>,
        _flags: &FhsFlags,
        core_output_formats: &[OutputFormat],
        heat_balance: bool,
        detailed_output_heating_cooling: bool,
        custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
    ) -> anyhow::Result<Option<HemResponse>> {
        FHS_COMPLIANCE_CALCULATIONS
            .par_iter()
            .map(|(key, flag)| {
                do_fhs_postprocessing(
                    output,
                    &results[key],
                    flag,
                    core_output_formats,
                    heat_balance,
                    detailed_output_heating_cooling,
                    custom_energy_supply_factors,
                )?;
                Ok(())
            })
            .collect::<anyhow::Result<()>>()?;

        let compliance_result =
            CalculatedComplianceResult::try_from((results, custom_energy_supply_factors))?;
        let compliance_response = FhsComplianceResponse::build_from(&compliance_result)?;

        Ok(Some(HemResponse::new(compliance_response)))
    }
}

static FHS_COMPLIANCE_CALCULATIONS: LazyLock<[(CalculationKey, FhsFlags); 4]> =
    LazyLock::new(|| {
        [
            (CalculationKey::Fhs, FhsFlags::FHS),
            (CalculationKey::FhsFee, FhsFlags::FHS_FEE),
            (CalculationKey::FhsNotional, FhsFlags::FHS_NOTIONAL),
            (CalculationKey::FhsNotionalFee, FhsFlags::FHS_FEE_NOTIONAL),
        ]
    });

fn do_fhs_preprocessing(
    input: &mut InputForProcessing,
    custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
    flag: &FhsFlags,
) -> anyhow::Result<()> {
    // Validate ventilation rates against part F
    if flag.contains(FhsFlags::FHS) {
        validate_dwelling_ventilation(
            input.infiltration_ventilation_node()?,
            calc_tfa(input)?,
            input.number_of_bedrooms()?,
            input.number_of_habitable_rooms()?,
            input
                .number_of_wet_rooms()?
                .ok_or_else(|| anyhow!("Expected NumberOfWetRooms to be provided"))?,
            input.number_of_bathrooms()?,
            input.number_of_utility_rooms()?,
            input.number_of_sanitary_accommodations()?,
            input.storeys_in_dwelling()?,
            input.kitchen_extractor_hood_external()?,
        )?;
    }
    if flag.contains(FhsFlags::FHS_FEE_NOTIONAL) {
        apply_fhs_notional_preprocessing(input, custom_energy_supply_factors, true)?;
    }
    if flag.contains(FhsFlags::FHS_NOTIONAL) {
        apply_fhs_notional_preprocessing(input, custom_energy_supply_factors, false)?;
    }
    if flag.intersects(FhsFlags::FHS_FEE | FhsFlags::FHS_FEE_NOTIONAL) {
        apply_fhs_fee_preprocessing(input)?;
    }

    final_preprocessing(input)?;

    Ok(())
}

fn metric_postprocessing(input: &Input, core_response: &Output) -> anyhow::Result<Metrics> {
    let energy_by_fuel = by_fuel(input, &core_response.summary)?;

    Ok(Metrics {
        eer: energy_efficiency_rating(core_response.summary.total_floor_area, &energy_by_fuel),
    })
}

fn do_fhs_postprocessing(
    output_writer: &impl OutputWriter,
    results: &CalculationResult,
    flags: &FhsFlags,
    core_output_formats: &[OutputFormat],
    heat_balance: bool,
    detailed_output_heating_cooling: bool,
    custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
) -> anyhow::Result<Option<HemResponse>> {
    let input = &results.input.clone();
    let OutputCore {
        timestep_array,
        results_end_user,
        energy_import,
        energy_export,
        ..
    } = &results.output.core;

    let output_mode = <&FhsFlags as Into<CalculationKey>>::into(flags).as_str();

    let steps_in_hours = results.input.simulation_time.step;

    write_core_output_files(
        &results.output,
        results.input.as_ref(),
        output_writer,
        output_mode,
        core_output_formats,
        steps_in_hours,
        heat_balance,
        detailed_output_heating_cooling,
    )?;

    let metrics = metric_postprocessing(results.input.as_ref(), &results.output)?;
    let location_key = format!("{output_mode}_metrics");
    let no_prefix_output_writer = output_writer.with_file_template("{}.{}".into());
    let writer = no_prefix_output_writer.writer_for_location_key(location_key.as_str(), "json")?;
    let mut serializer = Serializer::with_formatter(writer, PrettyFormatter::with_indent(b"    "));
    if let Err(e) = metrics.serialize(&mut serializer) {
        error!("Could not write out metrics postprocess file: {}", e);
    }

    if flags.intersects(FhsFlags::FHS | FhsFlags::FHS_NOTIONAL) {
        let notional = flags.contains(FhsFlags::FHS_NOTIONAL);
        apply_fhs_postprocessing(
            input,
            output_writer,
            energy_import,
            energy_export,
            results_end_user,
            timestep_array,
            notional,
            output_mode,
            custom_energy_supply_factors,
        )?;
    } else if flags.intersects(FhsFlags::FHS_FEE | FhsFlags::FHS_FEE_NOTIONAL) {
        let OutputSummary {
            space_heat_demand_total,
            space_cool_demand_total,
            total_floor_area,
            ..
        } = results.output.summary;
        apply_fhs_fee_postprocessing(
            output_writer,
            total_floor_area,
            space_heat_demand_total,
            space_cool_demand_total,
            output_mode,
        )?;
    }

    Ok(None)
}
