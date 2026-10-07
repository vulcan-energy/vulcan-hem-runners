use crate::future_homes_standard::fhs_sleeved_dhn_validation::validate_sleeved_dhn;
use crate::future_homes_standard::fhs_storeys_validation::validate_storeys_in_building_and_dwelling;
use crate::future_homes_standard::fhs_window_validation::{
    validate_existence_of_window, validate_window_base_height_within_ventilation_zone,
};
use crate::future_homes_standard::future_homes_standard::initial_preprocessing;
use crate::future_homes_standard::input::{ingest_for_processing, InputForProcessing};
use crate::future_homes_standard::{FhsComplianceWrapper, FhsIndividualCalcWrapper};
use anyhow::bail;
use bitflags::{bitflags, bitflags_match};
pub use home_energy_model::errors::HemError;
use home_energy_model::errors::PostprocessingError;
use home_energy_model::input::{CustomEnergySourceFactor, Input};
pub use home_energy_model::output_writer::{OutputWriter, SinkOutputWriter};
pub use home_energy_model::read_weather_file;
use home_energy_model::read_weather_file::{
    epw_weather_data_to_external_conditions, ExternalConditions,
};
pub use home_energy_model::HemResponse;
pub use home_energy_model::HEM_VERSION;
pub use home_energy_model::HEM_VERSION_DATE;
use home_energy_model::{CalculationResult, OutputFormat};
use indexmap::IndexMap;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fmt::Debug;
use std::io::{BufReader, Cursor, Read};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use tracing::{error, instrument};

pub mod future_homes_standard;

pub const FHS_VERSION: &str = "1.0.0a7";
pub const FHS_VERSION_DATE: &str = "2026-03-09";

bitflags! {
    #[derive(Debug, PartialEq)]
    pub struct FhsFlags: u32 {
        const FHS = 0b1;
        const FHS_FEE = 0b10;
        const FHS_NOTIONAL = 0b100;
        const FHS_FEE_NOTIONAL = 0b1000;
        const FHS_COMPLIANCE = 0b10000;
    }
}

/// Common trait for a wrapper of the HEM methodology, which in its preprocessing stage is able to
/// produce inputs for the HEM core for a particular purpose, and in its postprocessing stage is
/// able to output data as a side effect as well as returning a JSON-serializable response that can
/// be consumed or sent out from an API.
pub(crate) trait HemWrapper {
    fn apply_preprocessing(
        &self,
        input: InputForProcessing,
        custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
        flags: &FhsFlags,
    ) -> anyhow::Result<HashMap<CalculationKey, InputForProcessing>>;

    #[allow(clippy::too_many_arguments)]
    fn apply_postprocessing(
        &self,
        output: &impl OutputWriter,
        results: &HashMap<CalculationKey, CalculationResult>,
        flags: &FhsFlags,
        core_output_formats: &[OutputFormat],
        heat_balance: bool,
        detailed_output_heating_cooling: bool,
        custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
    ) -> anyhow::Result<Option<HemResponse>>;
}

/// An enum to wrap the known wrappers that could be chosen for a given invocation.
pub enum ChosenWrapper {
    FhsIndividualCalc(FhsIndividualCalcWrapper),
    FhsCompliance(FhsComplianceWrapper),
}

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum CalculationKey {
    Fhs,
    FhsFee,
    FhsNotional,
    FhsNotionalFee,
}

impl CalculationKey {
    fn as_str(&self) -> &'static str {
        match self {
            CalculationKey::Fhs => "FHS",
            CalculationKey::FhsFee => "FHS_FEE",
            CalculationKey::FhsNotional => "FHS_notional",
            CalculationKey::FhsNotionalFee => "FHS_notional_FEE",
        }
    }
}

impl From<&FhsFlags> for CalculationKey {
    fn from(value: &FhsFlags) -> Self {
        bitflags_match!(*value, {
            FhsFlags::FHS => CalculationKey::Fhs,
            FhsFlags::FHS_FEE => CalculationKey::FhsFee,
            FhsFlags::FHS_NOTIONAL => CalculationKey::FhsNotional,
            FhsFlags::FHS_FEE_NOTIONAL => CalculationKey::FhsNotionalFee,
            _ => unreachable!("Unknown flag option(s): {:?}", value),
        })
    }
}

impl HemWrapper for ChosenWrapper {
    fn apply_preprocessing(
        &self,
        input: InputForProcessing,
        custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
        flags: &FhsFlags,
    ) -> anyhow::Result<HashMap<CalculationKey, InputForProcessing>> {
        match self {
            ChosenWrapper::FhsIndividualCalc(wrapper) => {
                wrapper.apply_preprocessing(input, custom_energy_supply_factors, flags)
            }
            ChosenWrapper::FhsCompliance(wrapper) => {
                wrapper.apply_preprocessing(input, custom_energy_supply_factors, flags)
            }
        }
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
        match self {
            ChosenWrapper::FhsIndividualCalc(wrapper) => wrapper.apply_postprocessing(
                output,
                results,
                flags,
                core_output_formats,
                heat_balance,
                detailed_output_heating_cooling,
                custom_energy_supply_factors,
            ),
            ChosenWrapper::FhsCompliance(wrapper) => wrapper.apply_postprocessing(
                output,
                results,
                flags,
                core_output_formats,
                heat_balance,
                detailed_output_heating_cooling,
                custom_energy_supply_factors,
            ),
        }
    }
}

#[instrument(skip_all)]
#[allow(clippy::too_many_arguments)]
pub fn run_wrappers(
    // TODO: consider if this should move to main.rs
    input: impl Read,
    output_writer: impl OutputWriter,
    external_conditions_data: Option<ExternalConditions>,
    tariff_data_file: Option<&str>,
    flags: &FhsFlags,
    preprocess_only: bool,
    heat_balance: bool,
    detailed_output_heating_cooling: bool,
    core_output_formats: &[OutputFormat],
) -> Result<Option<HemResponse>, HemError> {
    catch_unwind(AssertUnwindSafe(|| {
        #[instrument(skip_all)]
        fn ingest_input_and_start_preprocessing(
            input: impl Read,
            external_conditions_data: Option<&ExternalConditions>,
        ) -> anyhow::Result<InputForProcessing> {
            let mut input_for_processing = ingest_for_processing(input)?;

            let default_weather_file = epw_weather_data_to_external_conditions(BufReader::new(Cursor::new(include_str!("future_homes_standard/RAF_Bedford_01.epw"))));

            let default_external_conditions_data = match default_weather_file {
                Ok(data) => data,
                Err(_) => bail!("Could not parse the default weather file!"),
            };
            let external_conditions_data = external_conditions_data.unwrap_or(&default_external_conditions_data);
            input_for_processing
                .merge_external_conditions_data(external_conditions_data.into())?;

            // Validate dwelling storeys is not greater than building storeys
            validate_storeys_in_building_and_dwelling(&input_for_processing)?;
            // Validate sleeved DHN
            validate_sleeved_dhn(&input_for_processing)?;
            // Validate dwelling has at least one window for vent size calculation
            validate_existence_of_window(&input_for_processing)?;
            // Validate dwelling windows are above the ventilation zone base height
            validate_window_base_height_within_ventilation_zone(&input_for_processing)?;

            Ok(input_for_processing)
        }

        fn choose_wrapper(flags: &FhsFlags) -> ChosenWrapper {
            {
                if flags.contains(FhsFlags::FHS_COMPLIANCE) {
                    ChosenWrapper::FhsCompliance(FhsComplianceWrapper::new())
                } else {
                    ChosenWrapper::FhsIndividualCalc(FhsIndividualCalcWrapper::new())
                }
            }
        }

        #[instrument(skip_all)]
        fn apply_preprocessing_from_wrappers(
            input_for_processing: InputForProcessing,
            custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
            wrapper: &impl HemWrapper,
            flags: &FhsFlags,
        ) -> anyhow::Result<HashMap<CalculationKey, InputForProcessing>> {
            wrapper.apply_preprocessing(input_for_processing, custom_energy_supply_factors, flags)
        }

        #[instrument(skip_all)]
        fn write_preproc_file(input: &Input, output: &impl OutputWriter, location_key: &str, file_extension: &str) -> anyhow::Result<()> {
            let writer = output.writer_for_location_key(location_key, file_extension)?;
            if let Err(e) = serde_json::to_writer_pretty(writer, input) {
                error!("Could not write out preprocess file: {}", e);
            }

            Ok(())
        }

        // 1. ingest / parse input and enter preprocessing stage
        let mut input_for_processing =
            ingest_input_and_start_preprocessing(input, external_conditions_data.as_ref())?;

        // 2. apply preprocessing from wrappers
        let wrapper = choose_wrapper(flags);

        let custom_energy_supply_factors = initial_preprocessing(&mut input_for_processing)?;

        let inputs_by_key = match catch_unwind(AssertUnwindSafe(|| {
            apply_preprocessing_from_wrappers(input_for_processing, &custom_energy_supply_factors, &wrapper, flags)
                .map_err(HemError::InvalidRequest)
        })) {
            Ok(result) => result?,
            Err(panic) => {
                return Err(HemError::PanicInWrapper(
                    panic
                        .downcast_ref::<&str>()
                        .map_or("Error not captured", |v| v)
                        .to_owned(),
                ))
            }
        };

        // 2b.(!) If preprocess-only flag is present and there is a primary calculation key, write out preprocess file
        if preprocess_only {
            for (calculation_key, input_for_processing) in inputs_by_key {
                let location_key = format!("{}__preproc", calculation_key.as_str());
                write_preproc_file(&input_for_processing.finalize()?, &output_writer, &location_key, "json")?;
            }

            return Ok(None);
        }

        let contextualised_results: Result<HashMap<CalculationKey, CalculationResult>, HemError> =
            inputs_by_key.par_iter()
                .map(|(key, input_value)| {
                    let finalized = input_value.clone().finalize()?; // TODO avoid cloning here!
                    home_energy_model::run_project(finalized, external_conditions_data.clone(), tariff_data_file, heat_balance, detailed_output_heating_cooling)
                        .map(|result_value| (*key, result_value))
                    }).collect();

        // 7. Run wrapper post-processing and capture any output.
        #[instrument(skip_all)]
        #[allow(clippy::too_many_arguments)]
        fn run_wrapper_postprocessing(
            output: &impl OutputWriter,
            results: &HashMap<CalculationKey, CalculationResult>,
            wrapper: &impl HemWrapper,
            flags: &FhsFlags,
            core_output_formats: &[OutputFormat],
            heat_balance: bool,
            detailed_output_heating_cooling: bool,
            custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
        ) -> anyhow::Result<Option<HemResponse>> {
                wrapper.apply_postprocessing(output, results, flags, core_output_formats, heat_balance, detailed_output_heating_cooling, custom_energy_supply_factors)
        }

        run_wrapper_postprocessing(&output_writer, &contextualised_results?, &wrapper, flags, core_output_formats, heat_balance, detailed_output_heating_cooling, &custom_energy_supply_factors)
            .map_err(|e| HemError::ErrorInPostprocessing(PostprocessingError::new(e)))
    }))
        .map_err(|e| {
            HemError::GeneralPanic(
                e.downcast_ref::<&str>()
                    .map_or("Uncaught panic - could not capture more information; to debug further, try replicating in debug mode.", |v| v)
                    .to_owned(),
            )
        })?
}
