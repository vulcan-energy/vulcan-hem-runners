use crate::future_homes_standard::fhs_appliance::FhsAppliance;
use crate::future_homes_standard::fhs_hw_events::{
    reset_events_and_provide_drawoff_generator, HotWaterEventGenerator,
};
use crate::future_homes_standard::fhs_imev_scheduler::create_imev_pattern;
use crate::future_homes_standard::input::{
    json_error, set_control_max_name_for_heat_source, set_control_min_name_for_heat_source,
    HotWaterSourceDetailsForProcessing, HotWaterSourceDetailsJsonMap, InputForProcessing,
    JsonAccessResult,
};
use anyhow::{anyhow, bail};
use csv::{Reader, WriterBuilder};
use home_energy_model::core::units::{
    Orientation360, DAYS_IN_MONTH, DAYS_PER_YEAR, HOURS_PER_DAY, WATTS_PER_KILOWATT,
};
use home_energy_model::hem_core::external_conditions::{
    create_external_conditions, ExternalConditions, WindowShadingObject,
};
use home_energy_model::hem_core::simulation_time::SimulationTime;
use home_energy_model::input::{
    CustomEnergySourceFactor, EnergySupplyDetails, EnergySupplyType, FuelType, Input,
    TransparentBuildingElement, TransparentBuildingElementJsonValue, WaterHeatingEventType,
};
use home_energy_model::output_writer::OutputWriter;
use indexmap::IndexMap;
use itertools::{izip, Itertools};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use smartstring::alias::String;
use std::convert::Into;
use std::io::{BufReader, Cursor, Read};
use std::iter::repeat_n;
use std::marker::PhantomData;
use std::sync::{Arc, LazyLock};

const HOURS_TO_END_DEC: f64 = 8760.;

pub(crate) const ENERGY_SUPPLY_NAME_GAS: &str = "mains gas";
pub(crate) const ENERGY_SUPPLY_NAME_ELECTRICITY: &str = "mains elec";
const APPL_OBJ_NAME: &str = "appliances";
const ELEC_COOK_OBJ_NAME: &str = "Eleccooking";
const GAS_COOK_OBJ_NAME: &str = "Gascooking";

const CLOTHES_WASHING_APPLIANCE: &str = "Clothes_washing";
const CLOTHES_DRYING_APPLIANCE: &str = "Clothes_drying";
const LAUNDRY_APPLIANCE_NAMES: [&str; 2] = [CLOTHES_WASHING_APPLIANCE, CLOTHES_DRYING_APPLIANCE];

pub(super) const LIVING_ROOM_SETPOINT_FHS: f64 = 21.0;
pub(super) const REST_OF_DWELLING_SETPOINT_FHS: f64 = 20.0;

pub(crate) const SIMTIME_START: f64 = 0.;
pub(crate) const SIMTIME_END: f64 = 8760.;
pub(crate) const SIMTIME_STEP: f64 = 0.5;
fn simtime() -> SimulationTime {
    SimulationTime::new(SIMTIME_START, SIMTIME_END, SIMTIME_STEP)
}

// Central point for hot water temperature (temp_hot_water) across the code
pub(crate) const HW_TEMPERATURE: f64 = 52.0;
const HW_SETPOINT_MAX: f64 = 60.0;

// Occupant sleep+wake hours as per Part O
const OCCUPANT_WAKING_HR: usize = 7;
const OCCUPANT_SLEEPING_HR: usize = 23;

/// Apply initial pre-processing required for all modes
pub(crate) fn initial_preprocessing(
    input: &mut InputForProcessing,
) -> anyhow::Result<IndexMap<Arc<str>, CustomEnergySourceFactor>> {
    create_hot_water_demand(input)?;
    create_zone_area(input)?;
    create_hot_water_distribution(input)?;
    let custom_energy_supply_factors = create_custom_energy_supply_factors(input)?;

    apply_defaults(input)?;

    input.reset_control()?;
    input.set_simulation_time(simtime())?;
    input.set_cross_vent_possible(true)?;

    Ok(custom_energy_supply_factors)
}

/// Apply assumptions and pre-processing steps for the Future Homes Standard
pub(crate) fn final_preprocessing(
    input: &mut InputForProcessing,
) -> anyhow::Result<&InputForProcessing> {
    input.reset_internal_gains()?;

    let tfa = calc_tfa(input)?;
    let nbeds = calc_nbeds(input)?;
    let n_occupants = calc_n_occupants(tfa, nbeds)?;

    // construct schedules
    let (_schedule_occupancy_weekday, _schedule_occupancy_weekend) =
        create_occupancy(n_occupants, APPLIANCE_PROPENSITIES.occupied);
    create_metabolic_gains(n_occupants, input)?;
    create_space_heat_distribution(input)?;
    create_water_heating_pattern(input)?;
    create_heating_pattern(input)?;
    create_charging_pattern(input)?;
    create_evaporative_losses(input, tfa, n_occupants, &EVAP_PROFILE_DATA)?;
    create_cold_water_losses(input, tfa, n_occupants, &COLD_WATER_LOSS_PROFILE_DATA)?;
    create_lighting_gains(input, tfa, n_occupants)?;
    create_appliance_gains(input, tfa, n_occupants, &APPLIANCE_PROPENSITIES)?;

    for (_, hw_source) in input.hot_water_source_mut()? {
        let hw_source = hw_source
            .as_object_mut()
            .ok_or_else(|| anyhow!("Hot water source is not an object"))?;

        let hw_source_type = hw_source
            .get("type")
            .ok_or_else(|| anyhow!("Type not found on hot water source"))?
            .as_str()
            .ok_or_else(|| anyhow!("Type field on hot water source is not a string"))?
            .to_owned();

        if hw_source_type == "StorageTank" {
            hw_source.insert("init_temp".into(), json!(HW_SETPOINT_MAX));
        } else if hw_source_type == "SmartHotWaterTank" {
            hw_source.insert("init_temp".into(), json!(HW_SETPOINT_MAX));
            hw_source.insert("temp_usable".into(), json!(HW_TEMPERATURE));
        } else if ["PointOfUse", "CombiBoiler", "HIU", "HeatBattery"]
            .contains(&hw_source_type.as_str())
        {
            hw_source.insert("setpoint_temp".into(), json!(HW_TEMPERATURE));
        }
    }

    {
        let cold_water_feed_temps = create_cold_water_feed_temps(input)?;
        create_hot_water_use_pattern(input, tfa, n_occupants, &cold_water_feed_temps)?;
    }
    create_cooling(input)?;
    create_window_opening_schedule(input)?;
    create_vent_opening_schedule(input)?;
    window_treatment(input)?;
    create_thermal_penetration(input)?;
    create_heating(input)?;
    create_infiltration_ventilation(input)?;
    calc_sfp_mech_vent(input)?;
    create_imev_pattern(input, SIMTIME_START, SIMTIME_END, SIMTIME_STEP)?;

    set_temp_internal_static_calcs(input)?;

    // Remove project_dict items that are not permitted by the core schema
    remove_fhs_only_inputs(input)?;
    Ok(input)
}

static APPLIANCE_PROPENSITIES: LazyLock<AppliancePropensities<Normalised>> = LazyLock::new(|| {
    load_appliance_propensities(Cursor::new(include_str!("./appliance_propensities.csv")))
        .expect("Could not read and parse appliance_propensities.csv")
});

static EVAP_PROFILE_DATA: LazyLock<HalfHourWeeklyProfileData> = LazyLock::new(|| {
    load_evaporative_profile(Cursor::new(include_str!("./evap_loss_profile.csv")))
        .expect("Could not read evap_loss_profile.csv.")
});

static COLD_WATER_LOSS_PROFILE_DATA: LazyLock<HalfHourWeeklyProfileData> = LazyLock::new(|| {
    load_evaporative_profile(Cursor::new(include_str!("./cold_water_loss_profile.csv")))
        .expect("Could not read cold_water_loss_profile.csv")
});

fn apply_defaults(input: &mut InputForProcessing) -> anyhow::Result<()> {
    for building_element in input.all_building_element_values_mut()? {
        let building_element_type = building_element
            .get("type")
            .and_then(|el| el.as_str())
            .ok_or_else(|| anyhow!("Building element type not found"))?;
        match building_element_type {
            "BuildingElementGround" => {
                building_element["pitch"] = json!(180);
            }
            "BuildingElementOpaque" => {
                let pitch = building_element
                    .get("pitch")
                    .and_then(|el| el.as_f64())
                    .ok_or_else(|| anyhow!("Building element pitch not found"))?;
                if !(60. ..=120.).contains(&pitch) {
                    building_element["colour"] = json!("Intermediate");
                }
                if [0., 180.].contains(&pitch) {
                    building_element["orientation360"] = json!(180);
                }
            }
            "BuildingElementTransparent" => {
                let pitch = building_element
                    .get("pitch")
                    .and_then(|el| el.as_f64())
                    .ok_or_else(|| anyhow!("Building element pitch not found"))?;
                if [0., 180.].contains(&pitch) {
                    building_element["orientation360"] = json!(180);
                }
            }
            _ => {}
        }
    }
    input
        .infiltration_ventilation_node_mut()?
        .insert("vent_opening_ratio_init".into(), json!(1));
    for vent in input.vents_mut()?.values_mut() {
        vent["pressure_difference_ref"] = json!(20);
    }
    for mech_vent in input.mechanical_ventilations_for_processing()? {
        mech_vent.insert("sup_air_flw_ctrl".into(), json!("ODA"));
        mech_vent.insert("sup_air_temp_ctrl".into(), json!("NO_CTRL"));
    }
    for energy_supply in input.energy_supplies_mut()? {
        if let Some(electric_battery) = energy_supply.get_mut("ElectricBattery") {
            electric_battery["battery_age"] = json!(0);
            electric_battery["grid_charging_possible"] = json!(false);
        }
        if energy_supply
            .get("fuel")
            .and_then(|fuel| fuel.as_str())
            .is_some_and(|fuel| ["mains_gas", "gas"].contains(&fuel))
        {
            energy_supply.insert("is_export_capable".into(), json!(false));
        } else if energy_supply
            .get("fuel")
            .and_then(|fuel| fuel.as_str())
            .is_some_and(|fuel| fuel == "electricity")
            && !energy_supply.contains_key("is_export_capable")
        {
            energy_supply.insert("is_export_capable".into(), json!(true));
        }
    }
    if let Some(baths) = input.baths_mut()? {
        for bath in baths.values_mut() {
            bath["flowrate"] = json!(12);
        }
    }
    for space_heat_system in input.space_heat_systems_mut()?.values_mut() {
        if space_heat_system
            .get("type")
            .and_then(|t| t.as_str())
            .is_some_and(|t| t == "ElecStorageHeater")
        {
            space_heat_system["state_of_charge_init"] = json!(1.0);
        }
    }
    if let Some(heat_source_wet) = input.heat_source_wet_mut()? {
        for heat_source in heat_source_wet.values_mut() {
            if let Some((Some(source_type), battery_type, backup_ctrl_type)) =
                heat_source.as_object().map(|source| {
                    (
                        source
                            .get("type")
                            .and_then(|t| t.as_str())
                            .map(ToOwned::to_owned),
                        source
                            .get("battery_type")
                            .and_then(|t| t.as_str())
                            .map(ToOwned::to_owned),
                        source
                            .get("backup_ctrl_type")
                            .and_then(|t| t.as_str())
                            .map(ToOwned::to_owned),
                    )
                })
            {
                {
                    if source_type == "HeatBattery" && battery_type.is_some_and(|bt| bt == "pcm") {
                        heat_source["temp_init"] = heat_source
                            .get("max_temperature")
                            .ok_or_else(|| anyhow!("max_temperature not found in heat source"))?
                            .clone();
                    }
                }
                if source_type == "HeatPump" && backup_ctrl_type.is_some_and(|bct| bct != "None") {
                    heat_source["time_delay_backup"] = json!(1.0);
                }
            }
        }
    }

    Ok(())
}

pub(super) fn set_temp_internal_static_calcs(input: &mut InputForProcessing) -> anyhow::Result<()> {
    input.set_temp_internal_air_static_calcs(Some(LIVING_ROOM_SETPOINT_FHS))?;

    Ok(())
}

static EMIS_PE_FACTORS: LazyLock<IndexMap<String, FactorData>> = LazyLock::new(|| {
    let mut factors: IndexMap<String, FactorData> = Default::default();

    let mut factors_reader = Reader::from_reader(BufReader::new(Cursor::new(include_str!(
        "./FHS_emisPEfactors_05-08-2024.csv"
    ))));
    for factor_data in factors_reader.deserialize() {
        let factor_data: FactorData = factor_data.expect("Reading the PE factors file failed.");
        if let Some(fuel_code) = &factor_data.fuel_code {
            factors.insert(fuel_code.clone(), factor_data);
        }
    }

    factors
});

static EMIS_PE_FACTORS_ELEC: LazyLock<IndexMap<usize, ElectricityFactorData>> =
    LazyLock::new(|| {
        // Load emissions factors and primary energy factors from data file for electricity
        let mut emis_pe_factors_elec: IndexMap<usize, ElectricityFactorData> = Default::default();

        let mut factors_reader = Reader::from_reader(BufReader::new(Cursor::new(include_str!(
            "./FHS_emisPEfactors_elec.csv"
        ))));

        for factor_data in factors_reader.deserialize() {
            let factor_data: ElectricityFactorData =
                factor_data.expect("Reading the PE factors elec file failed.");
            let timestep = &factor_data.timestep;
            emis_pe_factors_elec.insert(*timestep, factor_data);
        }

        emis_pe_factors_elec
    });

static METABOLIC_GAINS: LazyLock<MetabolicGains> = LazyLock::new(|| {
    let (weekday, weekend) = load_metabolic_gains_profile(Cursor::new(include_str!(
        "./dry_metabolic_gains_profile_Wperm2.csv"
    )))
    .expect("Could not load in metabolic gains file.");
    MetabolicGains { weekday, weekend }
});

struct MetabolicGains {
    weekday: [f64; 48],
    weekend: [f64; 48],
}

#[derive(Clone, Debug, Deserialize)]
struct FactorData {
    #[serde(rename = "Fuel Code")]
    fuel_code: Option<String>,
    #[serde(rename = "Fuel")]
    _fuel: String,
    #[serde(rename = "Emissions Factor kgCO2e/kWh")]
    emissions_factor: f64,
    #[serde(rename = "Emissions Factor kgCO2e/kWh including out-of-scope emissions")]
    emissions_factor_including_out_of_scope_emissions: f64,
    #[serde(rename = "Primary Energy Factor kWh/kWh delivered")]
    primary_energy_factor: f64,
}
#[derive(Clone, Debug, Deserialize)]
struct ElectricityFactorData {
    #[serde(rename = "Timestep")]
    timestep: usize,
    #[serde(rename = "Primary Energy Factor kWh/kWh delivered")]
    primary_energy_factor: f64,
    #[serde(rename = "Emissions Factor kgCO2e/kWh")]
    emissions_factor: f64,
    #[serde(rename = "Emissions Factor kgCO2e/kWh including out-of-scope emissions")]
    emissions_factor_including_out_of_scope_emissions: f64,
}

fn apply_energy_factor_series(energy_data: &[f64], factors: &Vec<f64>) -> anyhow::Result<Vec<f64>> {
    if energy_data.len() != factors.len() {
        bail!("Both energy_data and factors list must be of the same length.");
    }
    Ok(energy_data
        .iter()
        .zip(factors)
        .map(|(energy, factor)| energy * factor)
        .collect_vec())
}

#[allow(clippy::too_many_arguments)]
pub fn apply_fhs_postprocessing(
    input: &Input,
    output_writer: &impl OutputWriter,
    energy_import: &IndexMap<Arc<str>, Vec<f64>>,
    energy_export: &IndexMap<Arc<str>, Vec<f64>>,
    results_end_user: &IndexMap<Arc<str>, IndexMap<Arc<str>, Vec<f64>>>,
    timestep_array: &[f64],
    notional: bool,
    output_mode: &str,
    custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
) -> anyhow::Result<()> {
    let no_of_timesteps = timestep_array.len();

    let FinalRates {
        emission_rate: total_emissions_rate,
        primary_energy_rate: total_pe_rate,
        emissions_results: emis_results,
        emissions_out_of_scope_results: emis_oos_results,
        primary_energy_results: pe_results,
    } = calc_final_rates(
        input,
        energy_import,
        energy_export,
        results_end_user,
        no_of_timesteps,
        custom_energy_supply_factors,
    )?;

    // Write results to output files
    write_postproc_file(
        output_writer,
        output_mode,
        "emissions",
        emis_results,
        no_of_timesteps,
    )?;
    write_postproc_file(
        output_writer,
        output_mode,
        "emissions_incl_out_of_scope",
        emis_oos_results,
        no_of_timesteps,
    )?;
    write_postproc_file(
        output_writer,
        output_mode,
        "primary_energy",
        pe_results,
        no_of_timesteps,
    )?;
    write_postproc_summary_file(
        output_writer,
        output_mode,
        total_emissions_rate,
        total_pe_rate,
        notional,
    )?;

    Ok(())
}

pub(super) fn calc_final_rates(
    input: &Input,
    energy_import: &IndexMap<Arc<str>, Vec<f64>>,
    energy_export: &IndexMap<Arc<str>, Vec<f64>>,
    results_end_user: &IndexMap<Arc<str>, IndexMap<Arc<str>, Vec<f64>>>,
    number_of_timesteps: usize,
    custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
) -> anyhow::Result<FinalRates> {
    // For each EnergySupply object:
    // look up relevant factors for import/export from csv or custom factors
    // from input file
    // - look up relevant factors for generation from csv
    // - apply relevant factors for import, export and generation
    // Applying factors in this way rather than applying a net export factor to
    // exported energy accounts for energy generated and used on site and also
    // accounts for battery storage losses
    let mut emis_results: IndexMap<String, FhsCalculationResult> = Default::default();
    let mut emis_oos_results: IndexMap<String, FhsCalculationResult> = Default::default();
    let mut pe_results: IndexMap<String, FhsCalculationResult> = Default::default();

    for (energy_supply_key, energy_supply_details) in input
        .energy_supply()
        .iter()
        .map(|(key, value)| (key.clone(), value))
        // adding unmet demand to the energy supplies, rather than mutating the input as the Python does
        .chain(
            [(
                "_unmet_demand".into(),
                &EnergySupplyDetails::with_fuel(FuelType::UnmetDemand),
            )]
            .into_iter(),
        )
    {
        let energy_supply_key = String::from(energy_supply_key);
        let supply_emis_result = emis_results.entry(energy_supply_key.clone()).or_default();
        let supply_emis_oos_result = emis_oos_results
            .entry(energy_supply_key.clone())
            .or_default();
        let supply_pe_result = pe_results.entry(energy_supply_key.clone()).or_default();

        let fuel_code = energy_supply_details.fuel;

        let energy_supply_key: Arc<str> = energy_supply_key.as_str().into();

        // Get emissions/PE factors for import/export
        let (emis_factor_import_export, emis_oos_factor_import_export, pe_factor_import_export) =
            match fuel_code {
                FuelType::Custom => {
                    let custom_fuel_data = custom_energy_supply_factors[&energy_supply_key];
                    (
                        vec![custom_fuel_data.emissions_factor_kg_co2e_k_wh],
                        vec![
                            custom_fuel_data
                                .emissions_factor_kg_co2e_k_wh_including_out_of_scope_emissions,
                        ],
                        vec![custom_fuel_data.primary_energy_factor_k_wh_k_wh_delivered],
                    )
                }
                FuelType::Electricity => {
                    let emis_factor_import_export = EMIS_PE_FACTORS_ELEC
                        .values()
                        .map(|factor| factor.emissions_factor)
                        .collect_vec();
                    let emis_oos_factor_import_export = EMIS_PE_FACTORS_ELEC
                        .values()
                        .map(|factor| factor.emissions_factor_including_out_of_scope_emissions)
                        .collect_vec();
                    let pe_factor_import_export = EMIS_PE_FACTORS_ELEC
                        .values()
                        .map(|factor| factor.primary_energy_factor)
                        .collect_vec();
                    (
                        emis_factor_import_export,
                        emis_oos_factor_import_export,
                        pe_factor_import_export,
                    )
                }
                _ => {
                    let factor = EMIS_PE_FACTORS
                        .get(&String::from(fuel_code))
                        .ok_or_else(|| {
                            anyhow!("Expected factor values in the table for the fuel code {fuel_code} were not present.")
                        })?;
                    (
                        vec![factor.emissions_factor],
                        vec![factor.emissions_factor_including_out_of_scope_emissions],
                        vec![factor.primary_energy_factor],
                    )
                }
            };

        // Calculate energy imported and associated emissions/PE
        if fuel_code == FuelType::Electricity {
            supply_emis_result.import = apply_energy_factor_series(
                &energy_import[&energy_supply_key],
                &emis_factor_import_export,
            )?;
            supply_emis_oos_result.import = apply_energy_factor_series(
                &energy_import[&energy_supply_key],
                &emis_oos_factor_import_export,
            )?;
            supply_pe_result.import = apply_energy_factor_series(
                &energy_import[&energy_supply_key],
                &pe_factor_import_export,
            )?;
        } else if fuel_code == FuelType::UnmetDemand {
            // unmet demand is calculated as a special case where it is only the increase in unmet
            // demand between timesteps that should be accounted for, not the raw number

            let mut energy_import_increases =
                Vec::with_capacity(energy_import[&energy_supply_key].len());
            energy_import_increases.push(0.); // set up the first entry as we're going to use windows to perform inter-timestep comparisons
            for window in energy_import[&energy_supply_key].windows(2) {
                energy_import_increases.push((window[1] - window[0]).max(0.));
            }

            supply_emis_result.import = energy_import_increases
                .iter()
                .map(|x| x * emis_factor_import_export[0])
                .collect::<Vec<_>>();
            supply_emis_oos_result.import = energy_import_increases
                .iter()
                .map(|x| x * emis_oos_factor_import_export[0])
                .collect::<Vec<_>>();
            supply_pe_result.import = energy_import_increases
                .iter()
                .map(|x| x * pe_factor_import_export[0])
                .collect::<Vec<_>>();
        } else {
            supply_emis_result.import = energy_import[&energy_supply_key]
                .iter()
                .map(|x| x * emis_factor_import_export[0])
                .collect::<Vec<_>>();
            supply_emis_oos_result.import = energy_import[&energy_supply_key]
                .iter()
                .map(|x| x * emis_oos_factor_import_export[0])
                .collect::<Vec<_>>();
            supply_pe_result.import = energy_import[&energy_supply_key]
                .iter()
                .map(|x| x * pe_factor_import_export[0])
                .collect::<Vec<_>>();
        }

        // If there is any export, Calculate energy exported and associated emissions/PE
        // Note that by convention, exported energy is negative
        (
            supply_emis_result.export,
            supply_emis_oos_result.export,
            supply_pe_result.export,
        ) = if energy_export[&energy_supply_key].iter().sum::<f64>() < 0. {
            match fuel_code {
                FuelType::Electricity => (
                    apply_energy_factor_series(
                        &energy_export[&energy_supply_key],
                        &emis_factor_import_export,
                    )?,
                    apply_energy_factor_series(
                        &energy_export[&energy_supply_key],
                        &emis_oos_factor_import_export,
                    )?,
                    apply_energy_factor_series(
                        &energy_export[&energy_supply_key],
                        &pe_factor_import_export,
                    )?,
                ),
                FuelType::UnmetDemand => {
                    // unmet demand is calculated as a special case where it is only the decrease in
                    // unmet demand between timesteps that should be accounted for, not the raw number

                    let mut energy_import_decreases: Vec<f64> =
                        Vec::with_capacity(energy_export[&energy_supply_key].len());
                    energy_import_decreases.push(0.);
                    for window in energy_export[&energy_supply_key].windows(2) {
                        energy_import_decreases.push((window[1] - window[0]).min(0.));
                    }

                    (
                        energy_import_decreases
                            .iter()
                            .map(|x| x * emis_factor_import_export[0])
                            .collect::<Vec<_>>(),
                        energy_import_decreases
                            .iter()
                            .map(|x| x * emis_oos_factor_import_export[0])
                            .collect::<Vec<_>>(),
                        energy_import_decreases
                            .iter()
                            .map(|x| x * pe_factor_import_export[0])
                            .collect::<Vec<_>>(),
                    )
                }
                _ => (
                    energy_export[&energy_supply_key]
                        .iter()
                        .map(|x| x * emis_factor_import_export[0])
                        .collect::<Vec<_>>(),
                    energy_export[&energy_supply_key]
                        .iter()
                        .map(|x| x * emis_oos_factor_import_export[0])
                        .collect::<Vec<_>>(),
                    energy_export[&energy_supply_key]
                        .iter()
                        .map(|x| x * pe_factor_import_export[0])
                        .collect::<Vec<_>>(),
                ),
            }
        } else {
            (
                vec![0.; number_of_timesteps],
                vec![0.; number_of_timesteps],
                vec![0.; number_of_timesteps],
            )
        };

        // Calculate energy generated and associated emissions/PE
        let mut energy_generated = vec![0.; number_of_timesteps];
        for end_user_energy in results_end_user[&energy_supply_key].values() {
            if end_user_energy.iter().sum::<f64>() < 0. {
                for (t_idx, energy_generated_value) in energy_generated.iter_mut().enumerate() {
                    *energy_generated_value -= end_user_energy[t_idx];
                }
            }
        }

        (
            supply_emis_result.generated,
            supply_emis_oos_result.generated,
            supply_pe_result.generated,
        ) = if energy_generated.iter().sum::<f64>() > 0. {
            // TODO (from Python) Allow custom (user-defined) factors for generated energy?
            let generation_factors = EMIS_PE_FACTORS
                .get(&String::from("generation"))
                .unwrap_or_else(|| panic!("Generation row not found in the EMIS factors file."));
            let FactorData {
                emissions_factor: emis_factor_generated,
                emissions_factor_including_out_of_scope_emissions: emis_oos_factor_generated,
                primary_energy_factor: pe_factor_generated,
                ..
            } = generation_factors;

            if fuel_code == FuelType::UnmetDemand {
                // unmet demand is calculated as a special case where it is only the increase in unmet
                // demand between timesteps that should be accounted for, not the raw number

                let mut energy_generation_increases: Vec<f64> =
                    Vec::with_capacity(energy_generated.len());
                energy_generation_increases.push(0.);
                for window in energy_generated.windows(2) {
                    energy_generation_increases.push((window[1] - window[0]).max(0.));
                }

                (
                    energy_generation_increases
                        .iter()
                        .map(|x| x * emis_factor_generated)
                        .collect::<Vec<_>>(),
                    energy_generation_increases
                        .iter()
                        .map(|x| x * emis_oos_factor_generated)
                        .collect::<Vec<_>>(),
                    energy_generation_increases
                        .iter()
                        .map(|x| x * pe_factor_generated)
                        .collect::<Vec<_>>(),
                )
            } else {
                (
                    energy_generated
                        .iter()
                        .map(|x| x * emis_factor_generated)
                        .collect::<Vec<_>>(),
                    energy_generated
                        .iter()
                        .map(|x| x * emis_oos_factor_generated)
                        .collect::<Vec<_>>(),
                    energy_generated
                        .iter()
                        .map(|x| x * pe_factor_generated)
                        .collect::<Vec<_>>(),
                )
            }
        } else {
            (
                vec![0.; number_of_timesteps],
                vec![0.; number_of_timesteps],
                vec![0.; number_of_timesteps],
            )
        };

        // Calculate unregulated energy demand and associated emissions/PE
        let mut energy_unregulated = vec![0.; number_of_timesteps];
        for (end_user_name, end_user_energy) in results_end_user[&energy_supply_key].iter() {
            if [APPL_OBJ_NAME, ELEC_COOK_OBJ_NAME, GAS_COOK_OBJ_NAME]
                .contains(&end_user_name.as_ref())
            {
                for (t_idx, energy_unregulated_value) in energy_unregulated.iter_mut().enumerate() {
                    *energy_unregulated_value += end_user_energy[t_idx];
                }
            }
        }
        if fuel_code == FuelType::Electricity {
            supply_emis_result.unregulated =
                apply_energy_factor_series(&energy_unregulated, &emis_factor_import_export)?;
            supply_emis_oos_result.unregulated =
                apply_energy_factor_series(&energy_unregulated, &emis_oos_factor_import_export)?;
            supply_pe_result.unregulated =
                apply_energy_factor_series(&energy_unregulated, &pe_factor_import_export)?;
        } else if fuel_code == FuelType::UnmetDemand {
            // unmet demand is calculated as a special case where it is only the increase in unmet
            // demand between timesteps that should be accounted for, not the raw number

            let mut energy_unregulated_increases: Vec<f64> =
                Vec::with_capacity(energy_unregulated.len());
            energy_unregulated_increases.push(0.);
            for window in energy_unregulated.windows(2) {
                energy_unregulated_increases.push((window[1] - window[0]).max(0.));
            }

            supply_emis_result.unregulated = energy_unregulated_increases
                .iter()
                .map(|x| x * emis_factor_import_export[0])
                .collect::<Vec<_>>();
            supply_emis_oos_result.unregulated = energy_unregulated_increases
                .iter()
                .map(|x| x * emis_oos_factor_import_export[0])
                .collect::<Vec<_>>();
            supply_pe_result.unregulated = energy_unregulated_increases
                .iter()
                .map(|x| x * pe_factor_import_export[0])
                .collect::<Vec<_>>();
        } else {
            supply_emis_result.unregulated = energy_unregulated
                .iter()
                .map(|x| x * emis_factor_import_export[0])
                .collect::<Vec<_>>();
            supply_emis_oos_result.unregulated = energy_unregulated
                .iter()
                .map(|x| x * emis_oos_factor_import_export[0])
                .collect::<Vec<_>>();
            supply_pe_result.unregulated = energy_unregulated
                .iter()
                .map(|x| x * pe_factor_import_export[0])
                .collect::<Vec<_>>();
        }

        // Calculate total CO2/PE for each EnergySupply based on import and export,
        // subtracting unregulated
        supply_emis_result.total = Vec::with_capacity(number_of_timesteps);
        supply_emis_oos_result.total = Vec::with_capacity(number_of_timesteps);
        supply_pe_result.total = Vec::with_capacity(number_of_timesteps);
        for t_idx in 0..number_of_timesteps {
            supply_emis_result.total.push(
                supply_emis_result.import[t_idx]
                    + supply_emis_result.export[t_idx]
                    + supply_emis_result.generated[t_idx]
                    - supply_emis_result.unregulated[t_idx],
            );
            supply_emis_oos_result.total.push(
                supply_emis_oos_result.import[t_idx]
                    + supply_emis_oos_result.export[t_idx]
                    + supply_emis_oos_result.generated[t_idx]
                    - supply_emis_oos_result.unregulated[t_idx],
            );
            supply_pe_result.total.push(
                supply_pe_result.import[t_idx]
                    + supply_pe_result.export[t_idx]
                    + supply_pe_result.generated[t_idx]
                    - supply_pe_result.unregulated[t_idx],
            );
        }
    }

    let tfa = calc_tfa_from_finalised_input(input);
    let total_emissions_rate = emis_results
        .values()
        .map(|emis| emis.total.iter().sum::<f64>())
        .sum::<f64>()
        / tfa;
    let total_pe_rate = pe_results
        .values()
        .map(|pe| pe.total.iter().sum::<f64>())
        .sum::<f64>()
        / tfa;

    Ok(FinalRates {
        emission_rate: total_emissions_rate,
        primary_energy_rate: total_pe_rate,
        emissions_results: emis_results,
        emissions_out_of_scope_results: emis_oos_results,
        primary_energy_results: pe_results,
    })
}

pub(super) struct FinalRates {
    pub(super) emission_rate: f64,
    pub(super) primary_energy_rate: f64,
    pub(super) emissions_results: IndexMap<String, FhsCalculationResult>,
    pub(super) emissions_out_of_scope_results: IndexMap<String, FhsCalculationResult>,
    pub(super) primary_energy_results: IndexMap<String, FhsCalculationResult>,
}

#[derive(Default)]
pub(super) struct FhsCalculationResult {
    import: Vec<f64>,
    export: Vec<f64>,
    generated: Vec<f64>,
    unregulated: Vec<f64>,
    total: Vec<f64>,
}

impl FhsCalculationResult {
    fn labels(&self) -> [&'static str; 5] {
        ["import", "export", "generated", "unregulated", "total"]
    }

    fn printable_values_for_index(&self, index: usize) -> [String; 5] {
        // what's going on here can almost certainly be optimised
        [
            self.import[index].to_string().into(),
            self.export[index].to_string().into(),
            self.generated[index].to_string().into(),
            self.unregulated[index].to_string().into(),
            self.total[index].to_string().into(),
        ]
    }
}

fn write_postproc_file(
    output_writer: &impl OutputWriter,
    output_mode: &str,
    file_name: &str,
    results: IndexMap<String, FhsCalculationResult>,
    no_of_timesteps: usize,
) -> anyhow::Result<()> {
    let file_location = format!("{output_mode}__postproc_{file_name}");

    let mut row_headers: Vec<String> = Default::default();
    let mut rows_results: Vec<Vec<String>> = Default::default();

    // Loop over each EnergySupply object and add headers and results to rows
    for (energy_supply, energy_supply_results) in &results {
        for result_name in energy_supply_results.labels() {
            // Create header row
            row_headers.push(String::from([energy_supply, " ", result_name].concat()));
        }
    }

    // Create results rows
    for t_idx in 0..no_of_timesteps {
        let mut row = vec![];
        for energy_supply_results in results.values() {
            row.push(energy_supply_results.printable_values_for_index(t_idx));
        }
        rows_results.push(row.iter().flatten().cloned().collect());
    }

    let writer = output_writer.writer_for_location_key(&file_location, "csv")?;
    let mut writer = WriterBuilder::new().flexible(true).from_writer(writer);

    writer.write_record(row_headers)?;
    for record in rows_results {
        writer.write_record(record)?;
    }

    writer.flush()?;

    Ok(())
}

fn write_postproc_summary_file(
    output_writer: &impl OutputWriter,
    output_mode: &str,
    total_emissions_rate: f64,
    total_pe_rate: f64,
    notional: bool,
) -> anyhow::Result<()> {
    let (emissions_rate_name, pe_rate_name) = if notional {
        ("TER", "TPER")
    } else {
        ("DER", "DPER")
    };
    let file_location = format!("{output_mode}__postproc_summary");
    let writer = output_writer.writer_for_location_key(&file_location, "csv")?;
    let mut writer = WriterBuilder::new().flexible(true).from_writer(writer);

    writer.write_record(["", "", "Total"])?;
    writer.write_record([
        emissions_rate_name,
        "kgCO2/m2",
        total_emissions_rate.to_string().as_str(),
    ])?;
    writer.write_record([pe_rate_name, "kWh/m2", total_pe_rate.to_string().as_str()])?;

    writer.flush()?;

    Ok(())
}

pub(crate) fn calc_tfa(input: &InputForProcessing) -> JsonAccessResult<f64> {
    input.total_zone_area()
}

fn calc_tfa_from_finalised_input(input: &Input) -> f64 {
    input.total_floor_area()
}

pub(super) fn calc_nbeds(input: &InputForProcessing) -> anyhow::Result<usize> {
    Ok(input.number_of_bedrooms()?)
}

pub(super) fn calc_n_occupants(
    total_floor_area: f64,
    number_of_bedrooms: usize,
) -> anyhow::Result<f64> {
    if total_floor_area <= 0. {
        bail!("Invalid total floor area: {total_floor_area}, must be greater than 0");
    }

    // sigmoid curve is only used for one bedroom occupancy.
    // Therefore, sigmoid parameters only used if there is one bedroom
    Ok(match number_of_bedrooms {
        1 => {
            1. + ONE_BED_SIGMOID_PARAMS.j
                * (1. - (ONE_BED_SIGMOID_PARAMS.k * total_floor_area.powi(2)).exp())
        }
        2 => TWO_BED_OCCUPANCY,
        3 => THREE_BED_OCCUPANCY,
        4 => FOUR_BED_OCCUPANCY,
        n if n >= 5 => FIVE_BED_OCCUPANCY,
        _ => bail!("Invalid number of bedrooms: {number_of_bedrooms}"),
    })
}

struct SigmoidParams {
    j: f64,
    k: f64,
}

const ONE_BED_SIGMOID_PARAMS: SigmoidParams = SigmoidParams {
    j: 0.4373,
    k: -0.001902,
};
const TWO_BED_OCCUPANCY: f64 = 2.2472;
const THREE_BED_OCCUPANCY: f64 = 2.9796;
const FOUR_BED_OCCUPANCY: f64 = 3.3715;
const FIVE_BED_OCCUPANCY: f64 = 3.8997;

fn create_occupancy(n_occupants: f64, occupancy_fhs: [f64; 24]) -> ([f64; 24], [f64; 24]) {
    let schedule_occupancy_weekday = occupancy_fhs.map(|factor| factor * n_occupants);
    let schedule_occupancy_weekend = occupancy_fhs.map(|factor| factor * n_occupants);

    (schedule_occupancy_weekday, schedule_occupancy_weekend)
}

fn create_metabolic_gains(
    number_of_occupants: f64,
    input: &mut InputForProcessing,
    // NB. the Python includes two additional parameters here but they are unused
) -> anyhow::Result<()> {
    // Calculate total body surface area of occupants
    let a = 2.0001;
    let b = 0.8492;
    let total_body_surface_area_occupants = a * number_of_occupants.powf(b);

    let metabolic_gains_weekday_absolute = METABOLIC_GAINS
        .weekday
        .map(|gains| gains * total_body_surface_area_occupants)
        .to_vec();
    let metabolic_gains_weekend_absolute = METABOLIC_GAINS
        .weekend
        .map(|gains| gains * total_body_surface_area_occupants)
        .to_vec();

    input.set_metabolic_gains(
        0,
        0.5,
        json!(
            {
                "main": [{"repeat": 53, "value": "week"}],
                "week": [{"repeat": 5, "value": "weekday"}, {"repeat": 2, "value": "weekend"}],
                "weekday": metabolic_gains_weekday_absolute,
                "weekend": metabolic_gains_weekend_absolute,
            }
        ),
    )?;

    Ok(())
}

fn calc_zone_setpoint_fhs(zone: &JsonValue) -> anyhow::Result<f64> {
    let living_room_area = zone
        .get("livingroom_area")
        .and_then(JsonValue::as_f64)
        .ok_or_else(|| anyhow!("Living room area must be a valid number"))?;
    let rest_of_dwelling_area = zone
        .get("restofdwelling_area")
        .and_then(JsonValue::as_f64)
        .ok_or_else(|| anyhow!("Rest of dwelling area must be a valid number"))?;
    if living_room_area + rest_of_dwelling_area == 0. {
        bail!("Sum of living room area and rest of dwelling area must be greater than 0");
    }
    Ok((LIVING_ROOM_SETPOINT_FHS * living_room_area
        + REST_OF_DWELLING_SETPOINT_FHS * rest_of_dwelling_area)
        / (living_room_area + rest_of_dwelling_area))
}

fn load_metabolic_gains_profile(file: impl Read) -> anyhow::Result<([f64; 48], [f64; 48])> {
    let mut metabolic_gains_reader = Reader::from_reader(BufReader::new(file));
    let rows: Vec<DryMetabolicGainsRow> = metabolic_gains_reader
        .deserialize()
        .collect::<Result<Vec<DryMetabolicGainsRow>, _>>()?;
    Ok(rows
        .iter()
        .enumerate()
        .fold(([0.; 48], [0.; 48]), |mut acc, (i, item)| {
            acc.0[i] = item.weekday;
            acc.1[i] = item.weekend;
            acc
        }))
}

#[derive(Deserialize)]
#[serde(rename = "lowercase")]
struct DryMetabolicGainsRow {
    #[serde(rename = "half_hour")]
    _half_hour: usize,
    #[serde(alias = "Weekday")]
    weekday: f64,
    #[serde(alias = "Weekend")]
    weekend: f64,
}

fn habitable_building_height(input: &InputForProcessing) -> anyhow::Result<f64> {
    input.habitable_building_height().map_err(Into::into)
}

/// A pipework pre-processor module that calculates 22mm main distribution and 15mm heating circuit
/// lengths for modern buildings with internal shafts and two-pipe systems using BS 15316-3 Annex B
/// methodology.
///
///    Equations used for the installation scenario were defaulted to
///    "Shafts inside building (two-pipe)"
///
///    Args:
///        input: The main project dictionary where results are stored.
///
///    Effects:
///        Modifies the project_dict in-place by adding space heating distribution pipework.
fn create_space_heat_distribution(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let building_length = input.building_length()?;
    let building_width = input.building_width()?;
    let number_of_storeys = input.storeys_in_dwelling()?;
    let habitable_building_height = habitable_building_height(input)?;
    // Section V: 22 mm Mains
    // standard allowance per floor area for manifold take-offs and routing.
    let allowance_per_floor_area = 0.0325; //  m/m²
                                           // service allowance for connection spurs and access risers
    let service_allowance = 6.;
    let large_pipe_length = 2. * building_length
        + allowance_per_floor_area * building_length * building_width
        + service_allowance;
    // Section LS: 15 mm vertical shaft runs
    // pipe per volume of shaft, averaging out typical vertical layouts.
    let length_per_shaft_volume = 0.025; // m/m³
    let vertical_shaft_runs =
        length_per_shaft_volume * building_length * building_width * habitable_building_height;
    // Section LA: 15 mm lateral floor runs
    // extra return run needed in two-pipe loops versus the single loop in one-pipe systems.
    let return_run_factor = 0.55; // m/m²
    let lateral_floor_runs =
        return_run_factor * building_length * building_width * number_of_storeys as f64;
    let small_pipe_length = vertical_shaft_runs + lateral_floor_runs;

    for space_heat_system in input.space_heat_systems_mut()?.values_mut() {
        if space_heat_system
            .get("type")
            .and_then(JsonValue::as_str)
            .is_some_and(|type_str| type_str != "WetDistribution")
        {
            continue;
        }
        space_heat_system
            .as_object_mut()
            .ok_or_else(|| anyhow!("Failed to get space heat system as object"))?
            .insert(
                "pipework".into(),
                json!([
                    {
                        "insulation_thermal_conductivity": 0.035,
                        "insulation_thickness_mm": 0,
                        "external_diameter_mm": 15,
                        "internal_diameter_mm": 13,
                        "length": (small_pipe_length * 100.0).round() / 100.0,
                        "location": "internal",
                        "pipe_contents": "water",
                        "surface_reflectivity": false,
                    },
                    {
                        "insulation_thermal_conductivity": 0.035,
                        "insulation_thickness_mm": 0,
                        "external_diameter_mm": 22,
                        "internal_diameter_mm": 20,
                        "length": (large_pipe_length * 100.0).round() / 100.0,
                        "location": "internal",
                        "pipe_contents": "water",
                        "surface_reflectivity": false,
                    },
                ]),
            );
    }

    Ok(())
}

fn separate_temp_control_weekday_heating_schedule(
    zone: &JsonValue,
) -> anyhow::Result<[Option<f64>; 48]> {
    // 07:00-09:30 and then 16:30-22:00
    let mut heating_weekday = Vec::with_capacity(48);
    heating_weekday.extend(repeat_n(false, 14));
    heating_weekday.extend(repeat_n(true, 5));
    heating_weekday.extend(repeat_n(false, 14));
    heating_weekday.extend(repeat_n(true, 11));
    heating_weekday.extend(repeat_n(false, 4));
    let heating_weekday: [bool; 48] = heating_weekday.try_into().unwrap();
    let setpoint = calc_zone_setpoint_fhs(zone)?;

    Ok(heating_weekday.map(|is_heating| is_heating.then_some(setpoint)))
}

fn combined_schedule_setpoint(
    zone: &JsonValue,
    temp_setback: f64,
    heating_livingroom: bool,
    heating_restofdwelling: bool,
) -> anyhow::Result<f64> {
    let livingroom_area = zone
        .get("livingroom_area")
        .and_then(JsonValue::as_f64)
        .ok_or_else(|| anyhow!("Living room area not found in zone data"))?;
    let restofdwelling_area = zone
        .get("restofdwelling_area")
        .and_then(JsonValue::as_f64)
        .ok_or_else(|| anyhow!("Rest of dwelling area not found in zone data"))?;
    let livingroom_temp = if heating_livingroom {
        LIVING_ROOM_SETPOINT_FHS
    } else {
        temp_setback
    };
    let restofdwelling_temp = if heating_restofdwelling {
        REST_OF_DWELLING_SETPOINT_FHS
    } else {
        temp_setback
    };

    Ok(
        (livingroom_temp * livingroom_area + restofdwelling_temp * restofdwelling_area)
            / (livingroom_area + restofdwelling_area),
    )
}

fn separate_time_and_temp_control_weekday_heating_schedule(
    zone: &JsonValue,
    temp_setback: f64,
    advanced_start: f64,
) -> anyhow::Result<[Option<f64>; 48]> {
    if advanced_start > 7. {
        bail!("advanced_start exceeds 7 hours and is therefore incompatible with heating schedule starting at 07:00");
    }
    // Each hour of advanced start corresponds to 2 30 min timesteps
    let advanced_start_offset = (advanced_start * 2.) as usize;

    // 07:00-09:30 and then 16:30-22:00
    let mut heating_livingroom_weekday = Vec::with_capacity(48);
    heating_livingroom_weekday.extend(repeat_n(false, 14));
    heating_livingroom_weekday.extend(repeat_n(true, 5));
    heating_livingroom_weekday.extend(repeat_n(false, 14));
    heating_livingroom_weekday.extend(repeat_n(true, 11));
    heating_livingroom_weekday.extend(repeat_n(false, 4));
    let heating_livingroom_weekday: [bool; 48] = heating_livingroom_weekday.try_into().unwrap();

    // Adjusted livingroom schedule taking into account advanced start
    let mut heating_livingroom_weekday_with_advanced_start = Vec::with_capacity(48);
    heating_livingroom_weekday_with_advanced_start
        .extend(repeat_n(false, 14 - advanced_start_offset));
    heating_livingroom_weekday_with_advanced_start
        .extend(repeat_n(true, 5 + advanced_start_offset));
    heating_livingroom_weekday_with_advanced_start
        .extend(repeat_n(false, 14 - advanced_start_offset));
    heating_livingroom_weekday_with_advanced_start
        .extend(repeat_n(true, 11 + advanced_start_offset));
    heating_livingroom_weekday_with_advanced_start.extend(repeat_n(false, 4));
    let heating_livingroom_weekday_with_advanced_start: [bool; 48] =
        heating_livingroom_weekday_with_advanced_start
            .try_into()
            .unwrap();

    // 07:00-09:30 and then 18:30-22:00
    let mut heating_restofdwelling_weekday = Vec::with_capacity(48);
    heating_restofdwelling_weekday.extend(repeat_n(false, 14));
    heating_restofdwelling_weekday.extend(repeat_n(true, 5));
    heating_restofdwelling_weekday.extend(repeat_n(false, 18));
    heating_restofdwelling_weekday.extend(repeat_n(true, 7));
    heating_restofdwelling_weekday.extend(repeat_n(false, 4));
    let heating_restofdwelling_weekday: [bool; 48] =
        heating_restofdwelling_weekday.try_into().unwrap();

    // Adjusted restofdwelling schedule taking into account advanced start
    let mut heating_restofdwelling_weekday_with_advanced_start = Vec::with_capacity(48);
    heating_restofdwelling_weekday_with_advanced_start
        .extend(repeat_n(false, 14 - advanced_start_offset));
    heating_restofdwelling_weekday_with_advanced_start
        .extend(repeat_n(true, 5 + advanced_start_offset));
    heating_restofdwelling_weekday_with_advanced_start
        .extend(repeat_n(false, 18 - advanced_start_offset));
    heating_restofdwelling_weekday_with_advanced_start
        .extend(repeat_n(true, 7 + advanced_start_offset));
    heating_restofdwelling_weekday_with_advanced_start.extend(repeat_n(false, 4));
    let heating_restofdwelling_weekday_with_advanced_start: [bool; 48] =
        heating_restofdwelling_weekday_with_advanced_start
            .try_into()
            .unwrap();

    izip!(
        heating_livingroom_weekday_with_advanced_start,
        heating_restofdwelling_weekday_with_advanced_start,
        heating_livingroom_weekday,
        heating_restofdwelling_weekday
    )
    .map(
        |(
            heating_livingroom_with_advanced_start,
            heating_restofdwelling_with_advanced_start,
            heating_livingroom,
            heating_restofdwelling,
        )| {
            (heating_livingroom || heating_restofdwelling)
                .then(|| {
                    combined_schedule_setpoint(
                        zone,
                        temp_setback,
                        heating_livingroom_with_advanced_start,
                        heating_restofdwelling_with_advanced_start,
                    )
                })
                .transpose()
        },
    )
    .collect::<Result<Vec<Option<f64>>, _>>()?
    .try_into()
    .map_err(|_| anyhow!("Failed to convert to heating schedule with 48 entries"))
}

fn weekday_heating_schedule(
    zone: &JsonValue,
    temp_setback: f64,
    advanced_start: f64,
    heating_control_type: &str,
) -> anyhow::Result<[Option<f64>; 48]> {
    // The weekday schedule depends on the heating_control_type
    // because SeparateTimeAndTempControl means the livingroom and
    // restofdwelling can have different heating schedules so they need
    // to be combined to a suitably weighted temperature at each timestep.
    match heating_control_type {
        "SeparateTempControl" => separate_temp_control_weekday_heating_schedule(zone),
        "SeparateTimeAndTempControl" => separate_time_and_temp_control_weekday_heating_schedule(
            zone,
            temp_setback,
            advanced_start,
        ),
        _ => bail!("Invalid HeatingControlType: '{heating_control_type}', expected 'SeparateTempControl' or 'SeparateTimeAndTempControl'"),
    }
}

fn weekend_heating_schedule(zone: &JsonValue) -> anyhow::Result<[Option<f64>; 48]> {
    // 08:30 - 22:00
    let mut heating_weekend = Vec::with_capacity(48);
    heating_weekend.extend(repeat_n(false, 17));
    heating_weekend.extend(repeat_n(true, 27));
    heating_weekend.extend(repeat_n(false, 4));
    let heating_weekend: [bool; 48] = heating_weekend.try_into().unwrap();

    let setpoint = calc_zone_setpoint_fhs(zone)?;
    Ok(heating_weekend.map(|is_heating| is_heating.then_some(setpoint)))
}

/// Space heating.
fn create_heating_pattern(input: &mut InputForProcessing) -> anyhow::Result<()> {
    // Fixed heating setback temperature
    let temp_setback = 18.0;

    // Fixed advanced start
    let advanced_start = 2.;

    for zone_key in input.zone_keys()? {
        input.set_init_temp_setpoint_for_zone(
            &zone_key,
            calc_zone_setpoint_fhs(&json!(input.specific_zone(&zone_key)?))?,
        )?;
        let space_heat_systems = input.space_heat_system_for_zone(&zone_key)?;
        if space_heat_systems.is_empty() {
            continue;
        }
        for space_heat_system in space_heat_systems {
            let ctrlname = format!("HeatingPattern_{space_heat_system}");
            input.set_control_string_for_space_heat_system(&space_heat_system, &ctrlname)?;
            input.add_control(&ctrlname, json!({
                    "type": "SetpointTimeControl",
                    "start_day": 0,
                    "time_series_step": 0.5,
                    "schedule": {
                        "main": [{"repeat": 53, "value": "week"}],
                        "week": [
                            {"repeat": 5, "value": "weekday"},
                            {"repeat": 2, "value": "weekend"},
                        ],
                        "weekday": weekday_heating_schedule(
                            &json!(input.specific_zone(&zone_key)?), temp_setback, advanced_start, input.input.get("HeatingControlType").and_then(JsonValue::as_str).ok_or_else(|| anyhow!("HeatingControlType must be a valid string"))?
                        )?.to_vec(),
                        "weekend": weekend_heating_schedule(&json!(input.specific_zone(&zone_key)?))?.to_vec(),
                    },
                    "setpoint_min": temp_setback,
                    "advanced_start": advanced_start,
                }))?;
        }
    }

    Ok(())
}

/// Create charging control schedules for thermal storage systems.
///
/// This includes:
///    - Electric storage heaters
///    - Heat batteries
fn create_charging_pattern(input: &mut InputForProcessing) -> anyhow::Result<()> {
    // 00:00 until 07:00 off-peak charging
    let mut charging_offpeak = Vec::with_capacity(48);
    charging_offpeak.extend(repeat_n(true, 14));
    charging_offpeak.extend(repeat_n(false, 48 - 14));

    // Electric storage heaters (SpaceHeatSystem)
    for zone_key in input.zone_keys()? {
        let space_heat_systems = input.space_heat_system_for_zone(&zone_key)?;
        if space_heat_systems.is_empty() {
            continue;
        }
        for space_heat_system in space_heat_systems {
            if input
                .space_heat_system_for_key(&space_heat_system)?
                .and_then(|system| system.get("type"))
                .and_then(JsonValue::as_str)
                .is_some_and(|type_str| type_str == "ElecStorageHeater")
            {
                let charger_ctrlname = format!("ChargingPattern_{space_heat_system}");
                input.set_control_charger_for_space_heat_system(
                    &space_heat_system,
                    &charger_ctrlname,
                )?;
                input.add_control(
                    &charger_ctrlname,
                    json!({
                        "type": "ChargeControl",
                        "start_day": 0,
                        "time_series_step": 0.5,
                        "logic_type": "manual",
                        "charge_level": 1,
                        "schedule": {
                            "main": [{"value": "day", "repeat": 365}],
                            "day": charging_offpeak,
                        },
                    }),
                )?;
            }
        }
    }

    // Heat batteries (HeatSourceWet)
    // In addition any heat battery "HeatSourceWet" must have a ControlCharge
    for (heat_source_key, heat_source_wet) in input.heat_source_wet()? {
        if heat_source_wet
            .get("type")
            .and_then(JsonValue::as_str)
            .is_some_and(|type_str| type_str == "HeatBattery")
        {
            let hb_ctrlname = "HeatBattery_Control";
            input
                .heat_source_wet_by_key_mut(&heat_source_key)?
                .insert("ControlCharge".into(), json!(&hb_ctrlname));
            input.add_control(
                hb_ctrlname,
                json!({
                    "type": "ChargeControl",
                    "start_day": 0,
                    "time_series_step": 0.5,
                    "logic_type": "heat_battery",
                    "charge_level": 1,
                    "schedule": {
                        "main": [{"value": "day", "repeat": 365}],
                        "day": charging_offpeak,
                    },
                }),
            )?;
        }
    }

    Ok(())
}

/// water heating pattern - if system is not instantaneous, hold at setpoint
/// 00:00-02:00 every Sunday to allow for sterilisation cycle.
/// Note: Holding at setpoint for two hours has been chosen because
/// typical setting is for sterilisation cycle to last one hour, but the
/// model can only set a maximum and minimum setpoint temperaure, not
/// guarantee that the temperature is actually reached. Therefore, setting
/// the minimum to the maximum for two hours allows time for the tank
/// to heat up to the required temperature before being held there.
fn create_water_heating_pattern(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let hw_min_temp = "_HW_min_temp";
    let hw_max_temp = "_HW_max_temp";

    let hw_pv_diverter_max_temp_base_name = "_HW_pv_diverter_max_temp";
    let hw_pv_diverter_smart_hw_tank_ctrl_base_name = "_HW_pv_diverter_smart_hw_tank_ctrl";

    for energy_supply_name in input.names_of_energy_supplies_with_diverters()? {
        let hw_sources: IndexMap<_, _> = input
            .hot_water_source()?
            .iter()
            .map(|(name, source)| {
                (
                    String::from(name),
                    source
                        .get("type")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                )
            })
            .collect();
        for (hw_source_name, hw_source_type) in hw_sources {
            match hw_source_type.as_ref().map(|s| s.as_str()) {
                Some("StorageTank") => {
                    let control_name =
                        format!("{hw_pv_diverter_max_temp_base_name}_{hw_source_name}");
                    input.set_control_max_name_for_energy_supply_diverter(
                        &energy_supply_name,
                        &control_name,
                    )?;
                    input.add_control(
                        &control_name,
                        json!({
                            "type": "SetpointTimeControl",
                            "start_day": 0,
                            "time_series_step": 0.5,
                            "schedule": {
                                "main": [{"value": "day", "repeat": 365}],
                                "day": [
                                    {"value": HW_SETPOINT_MAX, "repeat": 48}
                                ]
                            }
                        }),
                    )?;
                }
                Some("SmartHotWaterTank") => {
                    let control_name =
                        format!("{hw_pv_diverter_smart_hw_tank_ctrl_base_name}_{hw_source_name}");
                    input.set_control_max_name_for_energy_supply_diverter(
                        &energy_supply_name,
                        &control_name,
                    )?;
                    input.add_control(
                        &control_name,
                        json!({
                            "type": "SetpointTimeControl",
                            "start_day": 0,
                            "time_series_step": 0.5,
                            "schedule": {
                                "main": [{"value": "day", "repeat": 365}],
                                "day": [
                                    {"value": 1.0, "repeat": 48}
                                ]
                            }
                        }),
                    )?;
                }
                _ => {}
            }
        }
    }

    input.add_control(
        hw_min_temp,
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 0.5,
            "schedule": {
                "main": [{"value": "week", "repeat": 53}],
                "week": [{"value": "other_day", "repeat": 6},{"value": "sunday", "repeat": 1}],
                "other_day": [{"value": HW_TEMPERATURE, "repeat": 48}],
                "sunday": [{"value": HW_SETPOINT_MAX, "repeat": 4},{"value": HW_TEMPERATURE, "repeat": 44}]
            }
        }),
    )?;
    input.add_control(
        hw_max_temp,
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 0.5,
            "schedule": {
                "main": [{"value": "day", "repeat": 365}],
                "day": [{"value": HW_SETPOINT_MAX, "repeat": 48}]
            }
        }),
    )?;

    let hw_smart_hot_water_tank_max_soc_name = "_HW_smart_hot_water_tank_max_soc";
    input.add_control(
        hw_smart_hot_water_tank_max_soc_name,
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 1,
            "schedule": {
                "day": [
                    {"value": 1.0, "repeat": 2},
                    {"value": 0.6, "repeat": 1},
                    {"value": 0.5, "repeat": 4},
                    {"value": 0.6, "repeat": 17}
                ],
                "main":[{"value": "day", "repeat": 365}]
                }
        }),
    )?;

    let hw_smart_hot_water_tank_min_soc_name = "_HW_smart_hot_water_tank_min_soc";
    input.add_control(
        hw_smart_hot_water_tank_min_soc_name,
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 1,
            "schedule": {
                "day": [
                    {"value": 1.0, "repeat": 2},
                    {"value": 0.1, "repeat": 1},
                    {"value": 0.5, "repeat": 4},
                    {"value": 0.1, "repeat": 17}
                ],
                "main":[{"value": "day", "repeat": 365}]
            }
        }),
    )?;

    let hw_smart_hot_water_tank_temp_max_name = "_HW_smart_hot_water_tank_temp_max";
    input.add_control(
        hw_smart_hot_water_tank_temp_max_name,
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 1,
            "schedule":{
                "main": [{"value": HW_SETPOINT_MAX, "repeat": 8760}]
            }
        }),
    )?;

    for mut hw_source in input
        .hot_water_source_mut()?
        .values_mut()
        .flat_map(|value| value.as_object_mut())
        .map(HotWaterSourceDetailsJsonMap)
    {
        if hw_source.is_storage_tank() {
            for heat_source in hw_source.all_heat_sources_mut()? {
                set_control_max_name_for_heat_source(heat_source, hw_max_temp)?;

                let heat_source_type = heat_source.get("type").ok_or_else(|| {
                    json_error("Type field missing from storage tank heat source")
                })?;

                if heat_source_type != "SolarThermalSystem" {
                    set_control_min_name_for_heat_source(heat_source, hw_min_temp)?;
                }
            }
        } else if hw_source.is_smart_hot_water_tank() {
            hw_source.set_temp_setpnt_max(hw_smart_hot_water_tank_temp_max_name);

            for heat_source in hw_source.all_heat_sources_mut()? {
                set_control_max_name_for_heat_source(
                    heat_source,
                    hw_smart_hot_water_tank_max_soc_name,
                )?;

                let heat_source_type = heat_source.get("type").ok_or_else(|| {
                    json_error("Type field missing from smart hot water tank heat source")
                })?;

                if heat_source_type != "SolarThermalSystem" {
                    set_control_min_name_for_heat_source(
                        heat_source,
                        hw_smart_hot_water_tank_min_soc_name,
                    )?;
                }
            }
        } else if hw_source.is_combi_boiler() || hw_source.is_point_of_use() || hw_source.is_hiu() {
            // Instantaneous water heating systems must be available 24 hours a day
            // so do nothing
        } else {
            bail!("Standard water heating schedule not defined for HotWaterSource type")
        }
    }

    if input.has_preheated_water_source()? {
        for heat_source in input.all_preheated_tank_heat_source_values_mut()? {
            let heat_source = heat_source
                .as_object_mut()
                .ok_or_else(|| json_error("Heat source on pre heated tank was not an object"))?;

            heat_source.insert("Controlmax".into(), hw_max_temp.into());

            let heat_source_type = heat_source
                .get("type")
                .ok_or_else(|| json_error("Type field missing from pre heated tank heat source"))?;

            if heat_source_type != "SolarThermalSystem" {
                heat_source.insert("Controlmin".into(), hw_min_temp.into());
            }
        }
    }

    Ok(())
}

/// Load the daily evaporative profile from a CSV file.
///
/// This function reads a CSV file containing time-of-day factors for evaporative losses
/// for each day of the week. It constructs a dictionary mapping days of the week to
/// lists of evaporative loss factors.
///
/// Arguments:
///
///  * `file` - The name of the CSV file containing the evaporative profile data.
///
///  Returns:
///     dict: A dictionary with days of the week as keys and lists of float factors as values.
fn load_evaporative_profile(file: impl Read) -> anyhow::Result<HalfHourWeeklyProfileData> {
    let mut profile_reader = Reader::from_reader(BufReader::new(file));

    let rows = profile_reader
        .deserialize()
        .collect::<Result<Vec<HalfHourWeeklyProfile>, _>>()
        .map_err(|_| anyhow!("Could not read evaporative profile file."))?;

    let (monday, tuesday, wednesday, thursday, friday, saturday, sunday) =
        rows.iter().enumerate().fold(
            (
                [0.; 48], [0.; 48], [0.; 48], [0.; 48], [0.; 48], [0.; 48], [0.; 48],
            ),
            |mut acc, (i, item)| {
                acc.0[i] = item.monday;
                acc.1[i] = item.tuesday;
                acc.2[i] = item.wednesday;
                acc.3[i] = item.thursday;
                acc.4[i] = item.friday;
                acc.5[i] = item.saturday;
                acc.6[i] = item.sunday;
                acc
            },
        );

    Ok(HalfHourWeeklyProfileData {
        monday,
        tuesday,
        wednesday,
        thursday,
        friday,
        saturday,
        sunday,
    })
}

#[derive(Debug, Deserialize)]
struct HalfHourWeeklyProfile {
    #[serde(rename = "Half_hour")]
    _half_hour: usize,
    #[serde(rename = "Mon")]
    monday: f64,
    #[serde(rename = "Tue")]
    tuesday: f64,
    #[serde(rename = "Wed")]
    wednesday: f64,
    #[serde(rename = "Thu")]
    thursday: f64,
    #[serde(rename = "Fri")]
    friday: f64,
    #[serde(rename = "Sat")]
    saturday: f64,
    #[serde(rename = "Sun")]
    sunday: f64,
}

struct HalfHourWeeklyProfileData {
    monday: [f64; 48],
    tuesday: [f64; 48],
    wednesday: [f64; 48],
    thursday: [f64; 48],
    friday: [f64; 48],
    saturday: [f64; 48],
    sunday: [f64; 48],
}

/// Apply the evaporative loss profile to modify the base evaporative loss across a full year.
///
/// This function takes the base evaporative loss and modifies it according to the provided
/// daily profile for each day of the week. It extends this profile throughout the year,
/// adjusting for any discrepancies in the week cycle (e.g., leap years).
///
/// Arguments:
///     * `input` - The main project dictionary where results are stored.
///     * `total_floor_area` - Total floor area used in the base loss calculation.
///     * `number_of_occupants` - Number of occupants used in the base loss calculation.
///     * `evaporative_profile_data` - Daily evaporative loss profiles loaded from a CSV file.
///
/// Effects:
///     Modifies the input in-place by setting a detailed schedule for evaporative losses.
fn create_evaporative_losses(
    input: &mut InputForProcessing,
    _total_floor_area: f64,
    number_of_occupants: f64,
    evaporative_profile_data: &HalfHourWeeklyProfileData,
) -> anyhow::Result<()> {
    // Base evaporative loss calculation
    let evaporative_losses_fhs = -25. * number_of_occupants;

    // Prepare to populate a full-year schedule of gains adjusted by the profile
    let mut evaporative_losses_schedule: Vec<f64> = Vec::with_capacity(18000);

    // Repeat for each week in a standard year
    evaporative_losses_schedule.extend(
        evaporative_profile_data
            .monday
            .iter()
            .chain(evaporative_profile_data.tuesday.iter())
            .chain(evaporative_profile_data.wednesday.iter())
            .chain(evaporative_profile_data.thursday.iter())
            .chain(evaporative_profile_data.friday.iter())
            .chain(evaporative_profile_data.saturday.iter())
            .chain(evaporative_profile_data.sunday.iter())
            .map(|factor| evaporative_losses_fhs * factor)
            .cycle()
            .take(48 * 7 * 52), // number of half-hour periods in 52 weeks
    );

    // Handle the extra days in the year not covered by the full weeks
    // Adjust based on the year (e.g., extra Monday for leap years)
    evaporative_losses_schedule.extend(
        evaporative_profile_data
            .monday
            .iter()
            .map(|factor| evaporative_losses_fhs * factor),
    );

    input.set_evaporative_losses(
        0,
        0.5,
        json!({
            "main": evaporative_losses_schedule,
        }),
    )?;

    Ok(())
}

/// Apply the cold water loss profile to modify the base cold water loss across a full year.
///
/// This function takes the base cold water loss and modifies it according to the provided
/// daily profile for each day of the week. It extends this profile throughout the year,
/// adjusting for any discrepancies in the weekly cycle (e.g., leap years).
///
/// Arguments:
///     * `input` - The main project dictionary where results are stored.
///     * `total_floor_area` - Total floor area used in the base loss calculation.
///     * `number_of_occupants` - Number of occupants used in the base loss calculation.
///     * `cold_water_loss_profile_data` - Daily cold water loss profiles loaded from a CSV file.
///
/// Effects:
///    Modifies the project_dict in-place by setting a detailed schedule for cold water losses.
fn create_cold_water_losses(
    input: &mut InputForProcessing,
    _total_floor_area: f64,
    number_of_occupants: f64,
    cold_water_loss_profile_data: &HalfHourWeeklyProfileData,
) -> anyhow::Result<()> {
    // Base cold water loss calculation
    let cold_water_losses_fhs = -20. * number_of_occupants;

    // Prepare to populate a full-year schedule of gains adjusted by the profile
    let mut cold_water_losses_schedule: Vec<f64> = Vec::with_capacity(18000);

    // Repeat for each week in a standard year
    cold_water_losses_schedule.extend(
        cold_water_loss_profile_data
            .monday
            .iter()
            .chain(cold_water_loss_profile_data.tuesday.iter())
            .chain(cold_water_loss_profile_data.wednesday.iter())
            .chain(cold_water_loss_profile_data.thursday.iter())
            .chain(cold_water_loss_profile_data.friday.iter())
            .chain(cold_water_loss_profile_data.saturday.iter())
            .chain(cold_water_loss_profile_data.sunday.iter())
            .map(|factor| cold_water_losses_fhs * factor)
            .cycle()
            .take(48 * 7 * 52), // number of half-hour periods in 52 weeks
    );

    // Handle the extra days in the year not covered by the full weeks
    // Adjust based on the year (e.g., extra Monday for leap years)
    cold_water_losses_schedule.extend(
        cold_water_loss_profile_data
            .monday
            .iter()
            .map(|factor| cold_water_losses_fhs * factor),
    );

    input.set_cold_water_losses(
        0,
        0.5,
        json!({
            "main": cold_water_losses_schedule
        }),
    )?;

    Ok(())
}

fn load_appliance_propensities(
    file: impl Read,
) -> anyhow::Result<AppliancePropensities<Normalised>> {
    let mut propensities_reader = Reader::from_reader(BufReader::new(file));
    let appliance_propensities_rows: Vec<AppliancePropensityRow> = propensities_reader
        .deserialize()
        .collect::<Result<Vec<AppliancePropensityRow>, _>>()
        .expect("Could not parse out appliance propensities CSV file correctly.");

    let (
        hour,
        occupied,
        cleaning_washing_machine,
        cleaning_tumble_dryer,
        cleaning_dishwasher,
        cooking_electric_oven,
        cooking_microwave,
        cooking_kettle,
        cooking_gas_cooker,
        consumer_electronics,
    ): AppliancePropensitiesUnderConstruction = appliance_propensities_rows
        .iter()
        .enumerate()
        .fold(Default::default(), |acc, (i, item)| {
            let (
                mut hour,
                mut occupied,
                mut cleaning_washing_machine,
                mut cleaning_tumble_dryer,
                mut cleaning_dishwasher,
                mut cooking_electric_oven,
                mut cooking_microwave,
                mut cooking_kettle,
                mut cooking_gas_cooker,
                mut consumer_electronics,
            ) = acc;
            hour[i] = item.hour as usize;
            occupied[i] = item.occupied;
            cleaning_washing_machine[i] = item.cleaning_washing_machine;
            cleaning_tumble_dryer[i] = item.cleaning_tumble_dryer;
            cleaning_dishwasher[i] = item.cleaning_dishwasher;
            cooking_electric_oven[i] = item.cooking_electric_oven;
            cooking_microwave[i] = item.cooking_microwave;
            cooking_kettle[i] = item.cooking_kettle;
            cooking_gas_cooker[i] = item.cooking_gas_cooker;
            consumer_electronics[i] = item.consumer_electronics;
            (
                hour,
                occupied,
                cleaning_washing_machine,
                cleaning_tumble_dryer,
                cleaning_dishwasher,
                cooking_electric_oven,
                cooking_microwave,
                cooking_kettle,
                cooking_gas_cooker,
                consumer_electronics,
            )
        });
    Ok(AppliancePropensities {
        hour,
        occupied,
        cleaning_washing_machine,
        cleaning_tumble_dryer,
        cleaning_dishwasher,
        cooking_electric_oven,
        cooking_microwave,
        cooking_kettle,
        cooking_gas_cooker,
        consumer_electronics,
        state: Default::default(),
    }
    .normalise())
}

type AppliancePropensitiesUnderConstruction = (
    [usize; 24],
    [f64; 24],
    [f64; 24],
    [f64; 24],
    [f64; 24],
    [f64; 24],
    [f64; 24],
    [f64; 24],
    [f64; 24],
    [f64; 24],
);

#[derive(Copy, Clone)]
struct AppliancePropensities<T> {
    hour: [usize; 24],
    occupied: [f64; 24],
    cleaning_washing_machine: [f64; 24],
    cleaning_tumble_dryer: [f64; 24],
    cleaning_dishwasher: [f64; 24],
    cooking_electric_oven: [f64; 24],
    cooking_microwave: [f64; 24],
    cooking_kettle: [f64; 24],
    cooking_gas_cooker: [f64; 24],
    consumer_electronics: [f64; 24],
    state: PhantomData<T>,
}

impl AppliancePropensities<AsDataFile> {
    fn normalise(self) -> AppliancePropensities<Normalised> {
        let AppliancePropensities {
            cleaning_washing_machine,
            cleaning_tumble_dryer,
            cleaning_dishwasher,
            cooking_electric_oven,
            cooking_microwave,
            cooking_kettle,
            cooking_gas_cooker,
            consumer_electronics,
            ..
        } = self;

        let [cleaning_washing_machine, cleaning_tumble_dryer, cleaning_dishwasher, cooking_electric_oven, cooking_microwave, cooking_kettle, cooking_gas_cooker, consumer_electronics] =
            [
                cleaning_washing_machine,
                cleaning_tumble_dryer,
                cleaning_dishwasher,
                cooking_electric_oven,
                cooking_microwave,
                cooking_kettle,
                cooking_gas_cooker,
                consumer_electronics,
            ]
            .into_iter()
            .map(|probabilities| -> [f64; 24] {
                let sumcol = probabilities.iter().sum::<f64>();
                probabilities.map(|x| x / sumcol)
            })
            .collect::<Vec<_>>()
            .try_into()
            .expect("Problem normalising appliance propensities.");

        AppliancePropensities {
            hour: self.hour,
            occupied: self.occupied,
            cleaning_washing_machine,
            cleaning_tumble_dryer,
            cleaning_dishwasher,
            cooking_electric_oven,
            cooking_microwave,
            cooking_kettle,
            cooking_gas_cooker,
            consumer_electronics,
            state: Default::default(),
        }
    }
}

struct AsDataFile;
struct Normalised;

#[derive(Deserialize)]
struct AppliancePropensityRow {
    #[serde(rename = "Hour")]
    hour: f64,
    #[serde(rename = "Occupied prop ( Chance the house is occupied)")]
    occupied: f64,
    #[serde(rename = "Cleaning Washing machine Prop")]
    cleaning_washing_machine: f64,
    #[serde(rename = "Cleaning Tumble dryer")]
    cleaning_tumble_dryer: f64,
    #[serde(rename = "Cleaning Dishwasher")]
    cleaning_dishwasher: f64,
    #[serde(rename = "Cooking Electric Oven")]
    cooking_electric_oven: f64,
    #[serde(rename = "Cooking Microwave")]
    cooking_microwave: f64,
    #[serde(rename = "Cooking Kettle")]
    cooking_kettle: f64,
    #[serde(rename = "Cooking Gas Cooker")]
    cooking_gas_cooker: f64,
    #[serde(rename = "Consumer Electronics")]
    consumer_electronics: f64,
}

struct FlatAnnualPropensities {
    cleaning_washing_machine: Vec<f64>,
    cleaning_tumble_dryer: Vec<f64>,
    cleaning_dishwasher: Vec<f64>,
    cooking_electric_oven: Vec<f64>,
    cooking_microwave: Vec<f64>,
    cooking_kettle: Vec<f64>,
    cooking_gas_cooker: Vec<f64>,
    consumer_electronics: Vec<f64>,
}

impl From<&AppliancePropensities<Normalised>> for FlatAnnualPropensities {
    fn from(value: &AppliancePropensities<Normalised>) -> Self {
        let hours_in_year = HOURS_TO_END_DEC as usize;
        Self {
            cleaning_washing_machine: value
                .cleaning_washing_machine
                .into_iter()
                .cycle()
                .take(hours_in_year)
                .collect::<Vec<_>>(),
            cleaning_tumble_dryer: value
                .cleaning_tumble_dryer
                .into_iter()
                .cycle()
                .take(hours_in_year)
                .collect::<Vec<_>>(),
            cleaning_dishwasher: value
                .cleaning_dishwasher
                .into_iter()
                .cycle()
                .take(hours_in_year)
                .collect::<Vec<_>>(),
            cooking_electric_oven: value
                .cooking_electric_oven
                .into_iter()
                .cycle()
                .take(hours_in_year)
                .collect::<Vec<_>>(),
            cooking_microwave: value
                .cooking_microwave
                .into_iter()
                .cycle()
                .take(hours_in_year)
                .collect::<Vec<_>>(),
            cooking_kettle: value
                .cooking_kettle
                .into_iter()
                .cycle()
                .take(hours_in_year)
                .collect::<Vec<_>>(),
            cooking_gas_cooker: value
                .cooking_gas_cooker
                .into_iter()
                .cycle()
                .take(hours_in_year)
                .collect::<Vec<_>>(),
            consumer_electronics: value
                .consumer_electronics
                .into_iter()
                .cycle()
                .take(hours_in_year)
                .collect::<Vec<_>>(),
        }
    }
}

/// Calculate the annual energy requirement in kWh using the procedure described in SAP 10.2 up to and including step 9.
/// Divide this by 365 to get the average daily energy use.
/// Multiply the daily energy consumption figure by the following profiles to
/// create a daily profile for each month of the year (to be applied to all days in that month).
/// Multiply by the daylighting at each half hourly timestep to correct for incidence of daylight.
fn create_lighting_gains(
    input: &mut InputForProcessing,
    total_floor_area: f64,
    number_of_occupants: f64,
) -> anyhow::Result<()> {
    // here we calculate an overall lighting efficacy as
    // the average of zone lighting efficacies weighted by zone
    // floor area.

    // Initialise variables for overall calculations
    let mut total_weighted_efficacy = 0.;
    let mut total_capacity = 0.;
    let mut total_area = 0.;

    if !input.all_zones_have_bulbs()? {
        bail!("At least one zone does not have lighting bulbs defined.");
    }
    for (zone_name, bulbs) in input.light_bulbs_for_each_zone()? {
        let mut zone_total_lumens = 0.0;
        let mut zone_total_wattage = 0.0;
        let mut zone_capacity = 0.0;

        for (i, bulb) in bulbs.iter().enumerate() {
            let bulb_efficacy = bulb
                .get("efficacy")
                .and_then(|e| e.as_f64())
                .ok_or(json_error(format!(
                "Bulb efficacy for bulb with index '{i}' should have been expressed as a number"
            )))?;
            let bulb_power = bulb
                .get("power")
                .and_then(|e| e.as_f64())
                .ok_or(json_error(format!(
                    "Bulb power for bulb with index '{i}' should have been expressed as a number"
                )))?;
            let bulb_count = bulb
                .get("count")
                .and_then(|e| e.as_u64())
                .ok_or(json_error(format!(
                    "Bulb count for bulb with index '{i}' should have been expressed as an integer"
                )))?;

            // Calculate total lumens and wattage for the bulb
            let bulb_lumens = bulb_efficacy * bulb_power * bulb_count as f64;
            let bulb_wattage = bulb_power * bulb_count as f64;
            let bulb_capacity = bulb_lumens;

            // Accumulate totals for the zone
            zone_total_lumens += bulb_lumens;
            zone_total_wattage += bulb_wattage;
            zone_capacity += bulb_capacity;
        }

        if zone_total_wattage == 0. {
            bail!("Invalid total wattage in zone {zone_name}, cannot equal 0.");
        }

        // Calculate zone efficacy
        let zone_efficacy = zone_total_lumens / zone_total_wattage;
        let zone_area = input.area_for_zone(&zone_name)?;

        // Accumulated weighted efficacy and capacities
        total_weighted_efficacy += zone_efficacy * zone_area;
        total_capacity += zone_capacity;
        total_area += zone_area;
    }

    if total_area == 0. {
        bail!("Invalid/missing value calculated for total area across zones, cannot equal 0.");
    }

    // Calculate overall lighting efficacy as area-weighted average
    let lighting_efficacy = total_weighted_efficacy / total_area;

    if lighting_efficacy == 0. {
        bail!(
            "Invalid lighting efficacy calculated from bulb details for all zones, cannot equal 0."
        );
    }

    // from analysis of EFUS 2017 data (updated to derive from harmonic mean)
    let lumens = 1_139. * (total_floor_area * number_of_occupants).powf(0.39);
    let mut topup = top_up_lighting(input, lumens, total_capacity)?;
    topup /= 21.3; // assumed efficacy of top up lighting
    let topup_per_day = topup / 365_f64;

    // dropped 1/3 - 2/3 split based on SAP2012 assumptions about portable lighting
    let kwh_per_year = lumens / lighting_efficacy;
    let kwh_per_day = kwh_per_year / 365.;
    let factor = daylight_factor(input, total_floor_area)?;

    // Need to expand the monthly profiles to get an annual profile
    let annual_half_hour_profile: Vec<f64> = DAYS_IN_MONTH
        .iter()
        .enumerate()
        .flat_map(|(month, days)| (0..*days).map(move |_| month))
        .flat_map(|month| AVERAGE_MONTHLY_LIGHTING_HALF_HOUR_PROFILES[month])
        .collect();

    // for each half hour time step in annual_halfhr_profiles:
    // To obtain the lighting gains,
    // the above should be converted to Watts by multiplying the individual half-hourly figure by (2 x 1000).
    // Since some lighting energy will be used in external light
    // (e.g. outdoor security lights or lights in unheated spaces like garages and sheds)
    // a factor of 0.85 is also applied to get the internal gains from lighting.
    let (lighting_gains_w, topup_gains_w): (Vec<f64>, Vec<f64>) = annual_half_hour_profile
        .into_iter()
        .enumerate()
        .map(|(i, profile)| {
            (
                (profile * kwh_per_day * factor[i]) * 2. * 1_000.,
                (profile * topup_per_day * factor[i]) * 2. * 1_000.,
            )
        })
        .collect();

    input.clear_appliance_gains()?;
    input.set_lighting_gains(json!({
        "start_day": 0,
        "time_series_step": 0.5,
        "gains_fraction": 0.85,
        "EnergySupply": ENERGY_SUPPLY_NAME_ELECTRICITY,
        "schedule": {
            "main": lighting_gains_w
        },
        "priority": -1
    }))?;

    input.set_topup_gains(json!({
        "start_day": 0,
        "time_series_step": 0.5,
        "gains_fraction": 0.85,
        "EnergySupply": ENERGY_SUPPLY_NAME_ELECTRICITY,
        "schedule": {
            "main": topup_gains_w
        }
    }))?;

    Ok(())
}

fn create_appliance_gains(
    input: &mut InputForProcessing,
    total_floor_area: f64,
    number_of_occupants: f64,
    appliance_propensities: &AppliancePropensities<Normalised>,
) -> anyhow::Result<()> {
    // take daily appliance use propensities and repeat them for one entire year
    let flat_annual_propensities: FlatAnnualPropensities = appliance_propensities.into();

    // add any missing required appliances to the assessment,
    // get default demand figures for any unknown appliances
    appliance_cooking_defaults(input, number_of_occupants, total_floor_area)?;
    let cookparams = cooking_demand(input, number_of_occupants)?;

    // TODO (from Python) change to enum
    // TODO (from Python) check appliances are named correctly and what to do if not?

    let appliance_map: IndexMap<&str, ApplianceUseProfile> = IndexMap::from([
        (
            "Fridge",
            ApplianceUseProfile::simple(
                1.,
                0.,
                1.0,
                vec![1. / HOURS_PER_DAY as f64; (HOURS_PER_DAY * DAYS_PER_YEAR) as usize],
            ),
        ),
        (
            "Freezer",
            ApplianceUseProfile::simple(
                1.,
                0.,
                1.0,
                vec![1. / HOURS_PER_DAY as f64; (HOURS_PER_DAY * DAYS_PER_YEAR) as usize],
            ),
        ),
        (
            "Fridge-Freezer",
            ApplianceUseProfile::simple(
                1.,
                0.,
                1.0,
                vec![1. / HOURS_PER_DAY as f64; (HOURS_PER_DAY * DAYS_PER_YEAR) as usize],
            ),
        ),
        (
            "Otherdevices",
            ApplianceUseProfile::simple(
                1.,
                0.,
                1.0,
                flat_annual_propensities.consumer_electronics.clone(),
            ),
        ),
        (
            "Dishwasher",
            ApplianceUseProfile::complex(
                number_of_occupants,
                132,       // HES 2012 final report table 22
                Some(280), // EU standard
                0.75,
                0.3,
                flat_annual_propensities.cleaning_dishwasher.clone(),
                1.5,
                0.,
            ),
        ),
        (
            "Clothes_washing",
            ApplianceUseProfile::clothes(
                number_of_occupants,
                174, // HES 2012 final report table 22
                220, // EU standard
                7.,
                0.75,
                0.3,
                flat_annual_propensities.cleaning_washing_machine.clone(),
                2.5,
                0.,
            ),
        ),
        (
            "Clothes_drying",
            ApplianceUseProfile::clothes(
                number_of_occupants,
                145, // HES 2012 final report table 22
                160, // EU standard
                7.,
                0.50,
                0.7,
                flat_annual_propensities.cleaning_tumble_dryer.clone(),
                0.75,
                0.,
            ),
        ),
        (
            "Oven",
            ApplianceUseProfile::complex(
                1.,
                cookparams.get("Oven").unwrap().event_count, // analysis of HES - see folder
                None,
                0.50,
                0.5,
                flat_annual_propensities.cooking_electric_oven.clone(),
                0.5,
                0.7,
            ),
        ),
        (
            "Hobs",
            ApplianceUseProfile::complex(
                1.,
                cookparams.get("Hobs").unwrap().event_count, // analysis of HES - see folder
                None,
                0.50,
                0.5,
                flat_annual_propensities.cooking_gas_cooker.clone(),
                0.1,
                0.7,
            ),
        ),
        (
            "Microwave",
            ApplianceUseProfile::complex(
                1.,
                cookparams.get("Microwave").unwrap().event_count, // analysis of HES - see folder
                None,
                0.50,
                1.,
                flat_annual_propensities.cooking_microwave.clone(),
                0.05,
                0.3,
            ),
        ),
        (
            "Kettle",
            ApplianceUseProfile::complex(
                1.,
                cookparams.get("Kettle").unwrap().event_count, // analysis of HES - see folder
                None,
                0.50,
                1.,
                flat_annual_propensities.cooking_kettle.clone(),
                0.05,
                0.3,
            ),
        ),
    ]);
    // add any missing required appliances to the assessment,
    // get default demand figures for any unknown appliances
    let mut appliance_kwhcycle: IndexMap<String, f64> = Default::default();

    let input_appliances = input.clone_appliances();

    // loop through appliances in the assessment.
    for (appliance_key, appliance) in input_appliances {
        // if it needs to be modelled per use
        let map_appliance = appliance_map
            .get(appliance_key.as_str())
            .expect("Appliance key was not in appliance map");

        if let Some(use_data) = map_appliance.use_data {
            // value on energy label is defined differently between appliance types
            // TODO (from Python) - translation of efficiencies should be its own function
            let (kwhcycle, loadingfactor) =
                appliance_kwh_cycle_loading_factor(input, &appliance_key, &appliance_map)?;

            let app = FhsAppliance::new(
                map_appliance.util_unit,
                use_data.use_metric as f64 * loadingfactor,
                kwhcycle,
                use_data.duration,
                map_appliance.standby,
                map_appliance.gains_frac,
                &map_appliance.prof,
                None,
                Some(use_data.duration_deviation),
            )?;

            let appliance_energy_supply = appliance.get("Energysupply").and_then(|e| e.as_str());

            let load_shifting = appliance.get("loadshifting").and_then(|v| v.as_object());

            input.set_gains_for_field(String::from(&appliance_key), json!({
                "EnergySupply": if ["Hobs", "Oven"].contains(&appliance_key.as_str()) {
                    appliance_energy_supply.ok_or_else(|| anyhow!("Could not get energy supply type for appliance with key {appliance_key}"))?.to_string()
                } else {
                    ENERGY_SUPPLY_NAME_ELECTRICITY.to_owned()
                },
                "start_day": 0,
                // TODO (from Python) - variable timestep
                "time_series_step": 1,
                "gains_fraction": app.gains_frac,
                "Events": app.event_list,
                "Standby": app.standby_w,
                "loadshifting": load_shifting
            }))?;

            appliance_kwhcycle.insert(appliance_key.into(), kwhcycle);
        } else {
            // model as yearlong time series schedule of demand in W
            let annual_kwh = match appliance.get("kWh_per_annum").and_then(|v| v.as_f64()) {
                Some(kwh) => kwh * map_appliance.util_unit,
                None => {
                    continue;
                }
            };

            let flat_schedule: Vec<f64> = appliance_map[appliance_key.as_str()]
                .prof
                .iter()
                .map(|&frac| WATTS_PER_KILOWATT as f64 / DAYS_PER_YEAR as f64 * frac * annual_kwh)
                .collect();

            let appliance_uses_gas: bool = false; // upstream Python checks appliance key contains substring 'gas', may be erroneous

            input.set_gains_for_field(String::from(&appliance_key), json!({
                "EnergySupply": if appliance_uses_gas { ENERGY_SUPPLY_NAME_GAS } else { ENERGY_SUPPLY_NAME_ELECTRICITY },
                "start_day": 0,
                "time_series_step": 1,
                "gains_fraction": map_appliance.gains_frac,
                "schedule": {
                   // watts
                   "main": flat_schedule
                }
            }))?;
        }
    }

    // Assign priority to those with a kWhcycle value, in reverse order
    for (priority, appliance) in appliance_kwhcycle
        // the Python behaviour differs here as it sorts by index 1 (2nd letter) of the appliance key string,
        // in the Rust we've implemented what we think is the intended behaviour and reported the bug to DESNZ
        .sorted_by(|_, v1, _, v2| v1.total_cmp(v2))
        .rev()
        .enumerate()
    {
        input.set_priority_for_gains_appliance(priority as isize, &appliance.0)?;
    }

    Ok(())
}

#[derive(Clone, Debug)]
struct ApplianceUseProfile {
    util_unit: f64,
    use_data: Option<ApplianceUseData>,
    standby: f64,
    gains_frac: f64,
    prof: Vec<f64>,
}

impl ApplianceUseProfile {
    fn simple(util_unit: f64, standby: f64, gains_frac: f64, prof: Vec<f64>) -> Self {
        Self {
            util_unit,
            use_data: None,
            standby,
            gains_frac,
            prof,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn complex(
        util_unit: f64,
        use_metric: usize,
        standard_use: Option<usize>,
        standby: f64,
        gains_frac: f64,
        prof: Vec<f64>,
        duration: f64,
        duration_deviation: f64,
    ) -> Self {
        Self {
            util_unit,
            use_data: Some(ApplianceUseData {
                use_metric,
                clothes_use_data: None,
                _standard_use: standard_use,
                duration,
                duration_deviation,
            }),
            standby,
            gains_frac,
            prof,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn clothes(
        util_unit: f64,
        use_metric: usize,
        standard_use: usize,
        standard_load_kg: f64,
        standby: f64,
        gains_frac: f64,
        prof: Vec<f64>,
        duration: f64,
        duration_deviation: f64,
    ) -> Self {
        Self {
            util_unit,
            use_data: Some(ApplianceUseData {
                use_metric,
                clothes_use_data: Some(ClothesUseData { standard_load_kg }),
                _standard_use: Some(standard_use),
                duration,
                duration_deviation,
            }),
            standby,
            gains_frac,
            prof,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ApplianceUseData {
    // maps to "use" field in upstream, though 'use' is a keywork in Rust so calling this "use_metric"
    use_metric: usize,
    clothes_use_data: Option<ClothesUseData>,
    _standard_use: Option<usize>,
    duration: f64,
    duration_deviation: f64,
}

#[derive(Clone, Copy, Debug)]
struct ClothesUseData {
    standard_load_kg: f64,
}
struct ApplianceCookingDemand {
    mean_annual_demand: f64,
    _mean_annual_events: f64,
    mean_event_demand: f64,
    fuel: Option<String>,
    event_count: usize,
}

fn cooking_demand(
    input: &mut InputForProcessing,
    number_of_occupants: f64,
) -> anyhow::Result<IndexMap<&str, ApplianceCookingDemand>> {
    let oven_energy_supply = input.energy_supply_for_appliance("Oven");
    let oven_fuel = match oven_energy_supply {
        Ok(energy_supply) => Some(input.fuel_type_for_energy_supply_reference(energy_supply)?),
        Err(_) => None,
    };
    let oven = ApplianceCookingDemand {
        mean_annual_demand: 285.14,
        _mean_annual_events: 441.11,
        mean_event_demand: 0.762,
        fuel: oven_fuel,
        event_count: Default::default(),
    };

    let hobs_energy_supply = input.energy_supply_for_appliance("Hobs");
    let hobs_fuel = match hobs_energy_supply {
        Ok(energy_supply) => Some(input.fuel_type_for_energy_supply_reference(energy_supply)?),
        Err(_) => None,
    };
    let hobs = ApplianceCookingDemand {
        mean_annual_demand: 352.53,
        _mean_annual_events: 520.86,
        mean_event_demand: 0.810,
        fuel: hobs_fuel,
        event_count: Default::default(),
    };

    let microwave_fuel = match input.appliances_contain_key("Microwave") {
        true => Some(EnergySupplyType::Electricity.into()),
        false => None,
    };
    let microwave = ApplianceCookingDemand {
        mean_annual_demand: 44.11,
        _mean_annual_events: 710.65,
        mean_event_demand: 0.0772,
        fuel: microwave_fuel,
        event_count: Default::default(),
    };

    let kettle_fuel = match input.appliances_contain_key("Kettle") {
        true => Some(EnergySupplyType::Electricity.into()),
        false => None,
    };
    let kettle = ApplianceCookingDemand {
        mean_annual_demand: 173.03,
        _mean_annual_events: 1782.5,
        mean_event_demand: 0.0985,
        fuel: kettle_fuel,
        event_count: Default::default(),
    };
    let mut cook_params = IndexMap::from([
        ("Oven", oven),
        ("Hobs", hobs),
        ("Microwave", microwave),
        ("Kettle", kettle),
    ]);

    let gas_total: f64 = cook_params
        .values()
        .filter(|appliance_details| {
            appliance_details
                .fuel
                .as_ref()
                .is_some_and(|fuel| *fuel == FuelType::MainsGas.to_string())
        })
        .map(|appliance_details| appliance_details.mean_annual_demand)
        .sum();

    let elec_total: f64 = cook_params
        .values()
        .filter(|appliance_details| {
            appliance_details
                .fuel
                .as_ref()
                .is_some_and(|fuel| *fuel == FuelType::Electricity.to_string())
        })
        .map(|appliance_details| appliance_details.mean_annual_demand)
        .sum();

    // top down cooking demand estimate based on analysis of EFUS 2017 electricity monitoring data
    // and HES 2012
    let annual_cooking_elec_kwh = 448. * 0.8 + (171. + 98. * number_of_occupants) * 0.2;

    for cooking_demand in cook_params.values_mut() {
        // for each appliance, work out number of usage events based on
        // average HES annual demand and demand per cycle
        // do not consider gas and electricity separately for this purpose
        let demand_prop = cooking_demand.mean_annual_demand / (elec_total + gas_total);
        let annual_kwh = demand_prop * annual_cooking_elec_kwh;
        let events = annual_kwh / cooking_demand.mean_event_demand;
        cooking_demand.event_count = events as usize;
    }

    Ok(cook_params)
}

type ApplianceCookingDefaults<'a> = (IndexMap<&'a str, JsonValue>, IndexMap<&'a str, JsonValue>);

fn appliance_cooking_defaults(
    input: &mut InputForProcessing,
    number_of_occupants: f64,
    total_floor_area: f64,
) -> anyhow::Result<ApplianceCookingDefaults<'_>> {
    let cooking_fuels = input.all_energy_supply_fuel_types()?;

    // (from Python) also check gas/elec cooker/oven  together - better to have energysupply as a dict entry?
    let mut cooking_defaults: IndexMap<&str, JsonValue> = match (
        cooking_fuels.contains("electricity"),
        cooking_fuels.contains("mains_gas"),
    ) {
        (true, true) => IndexMap::from([
            (
                "Oven",
                json!({
                    "Energysupply": "mains elec",
                    "kWh_per_cycle": 0.59,
                }),
            ),
            (
                "Hobs",
                json!({
                    "Energysupply": "mains gas",
                    "kWh_per_cycle": 0.72,
                }),
            ),
        ]),
        (_, true) => IndexMap::from([
            (
                "Oven",
                json!({
                    "Energysupply": "mains gas",
                    "kWh_per_cycle": 1.57,
                }),
            ),
            (
                "Hobs",
                json!({
                    "Energysupply": "mains gas",
                    "kWh_per_cycle": 0.72,
                }),
            ),
        ]),
        (true, _) => IndexMap::from([
            (
                "Oven",
                json!({
                    "Energysupply": "mains elec",
                    "kWh_per_cycle": 0.59,
                }),
            ),
            (
                "Hobs",
                json!({
                    "Energysupply": "mains elec",
                    "kWh_per_cycle": 0.72,
                }),
            ),
        ]),
        _ => IndexMap::from([
            (
                "Oven",
                json!({
                    "Energysupply": "mains elec",
                    "kWh_per_cycle": 0.59,
                }),
            ),
            (
                "Hobs",
                json!({
                    "Energysupply": "mains elec",
                    "kWh_per_cycle": 0.72,
                }),
            ),
        ]),
    };

    let mut additional_cooking_defaults = IndexMap::from([
        ("Kettle", json!({"kWh_per_cycle": 0.1})),
        ("Microwave", json!({"kWh_per_cycle": 0.08})),
    ]);

    let appliance_defaults = IndexMap::from([
        (
            "Otherdevices",
            json!({
                "kWh_per_annum": 30.0 * (number_of_occupants * total_floor_area).powf(0.49),
            }),
        ),
        ("Dishwasher", json!({"kWh_per_100cycle" : 53.0})),
        (
            "Clothes_washing",
            json!({
                "kWh_per_100cycle" : 53.0,
                "kg_load": 7.0
            }),
        ),
        (
            "Clothes_drying",
            json!({
                "kWh_per_100cycle" : 98.0,
                "kg_load": 7.0
            }),
        ),
        ("Fridge", json!({"kWh_per_annum" : 76.7})),
        ("Freezer", json!({"kWh_per_annum" : 128.2})),
        ("Fridge-Freezer", json!({"kWh_per_annum" : 137.4})),
    ]);

    if !input.has_appliances()? {
        input.merge_in_appliances(&appliance_defaults)?;
        input.merge_in_appliances(&cooking_defaults)?;
        input.merge_in_appliances(&additional_cooking_defaults)?;
    } else {
        for appliance_name in appliance_defaults.keys() {
            if !input.appliances_contain_key(appliance_name)
                || input.appliance_key_has_reference(appliance_name, "Default")?
            {
                input.merge_in_appliances(&IndexMap::from([(
                    appliance_name.to_owned(),
                    appliance_defaults[appliance_name].clone(),
                )]))?;
            } else if input.appliance_key_has_reference(appliance_name, "Not Installed")? {
                input.remove_appliance(appliance_name)?;
            } else {
                // user has specified appliance efficiency, overwrite efficiency with default
                let original_load_shifting_value =
                    input.loadshifting_for_appliance(appliance_name)?;

                input.merge_in_appliances(&IndexMap::from([(
                    appliance_name.to_owned(),
                    appliance_defaults[appliance_name].clone(),
                )]))?;
                if let Some(load_shifting) = original_load_shifting_value {
                    input.set_loadshifting_for_appliance(appliance_name, json!(load_shifting))?;
                }
            }
        }
        if !cooking_defaults
            .keys()
            .any(|cooking_appliance_name| input.appliances_contain_key(cooking_appliance_name))
        {
            // neither cooker nor oven specified, add cooker as minimum requirement
            input.merge_in_appliances(&IndexMap::from([(
                "Hobs",
                cooking_defaults["Hobs"].clone(),
            )]))?;
        }
        cooking_defaults.append(&mut additional_cooking_defaults);
        for (cooking_name, cooking_appliance) in cooking_defaults.iter() {
            if !input.appliances_contain_key(cooking_name)
                || input.appliance_key_has_reference(cooking_name, "Default")?
            {
                input.merge_in_appliances(&IndexMap::from([(
                    cooking_name.to_owned(),
                    cooking_appliance.clone(),
                )]))?;
            } else if input.appliance_key_has_reference(cooking_name, "Not Installed")? {
                input.remove_appliance(cooking_name)?;
            } else {
                // NB: there is a possible issue in the Python here where the wrong key is used
                input.merge_in_appliances(&IndexMap::from([(
                    cooking_name.to_owned(),
                    cooking_appliance.clone(),
                )]))?;
            }
        }
    }

    Ok((appliance_defaults, cooking_defaults))
}

fn appliance_kwh_cycle_loading_factor(
    input: &InputForProcessing,
    appliance_key: &str,
    appliance_map: &IndexMap<&str, ApplianceUseProfile>,
) -> anyhow::Result<(f64, f64)> {
    // value on energy label is defined differently between appliance types,
    // convert any different input types to simple kWh per cycle

    let appliance = input
        .appliance_with_key(appliance_key)?
        .ok_or_else(|| anyhow!("Appliance '{appliance_key}' not found"))?;
    let kwh_cycle = get_kwh_per_cycle(appliance, appliance_key)?;

    let (loading_factor, kwh_cycle) = if LAUNDRY_APPLIANCE_NAMES.contains(&appliance_key) {
        // additionally, laundry appliances have variable load size,
        // which affects the required number of uses to do all the occupants' laundry for the year
        let loading_factor = appliance_map.get(appliance_key).ok_or_else(|| {
            anyhow!(
                "Appliance '{appliance_key}' not found in map of known appliances.",
            )
        })?
        .use_data.and_then(|use_data| use_data.clothes_use_data).map(|clothes_use_data| clothes_use_data.standard_load_kg).ok_or_else(|| {
            anyhow!(
                "Appliance '{appliance_key}' has no standard_load_kg value, cannot calculate loading factor.",
            )
        })? / appliance.get("kg_load").and_then(|kg_load| kg_load.as_f64()).ok_or_else(|| anyhow!("Appliance '{appliance_key}' has no kg_load value, cannot calculate loading factor."))?;

        (
            loading_factor,
            if appliance_key == CLOTHES_DRYING_APPLIANCE {
                let residual_moisture_adjustment = get_residual_moisture_adjustment(input)?;
                kwh_cycle * residual_moisture_adjustment
            } else {
                kwh_cycle
            },
        )
    } else {
        (1.0, kwh_cycle)
    };

    Ok((kwh_cycle, loading_factor))
}

fn get_kwh_per_cycle(appliance: &JsonValue, appliance_name: &str) -> anyhow::Result<f64> {
    if let Some(kwh_per_cycle) = appliance.get("kWh_per_cycle") {
        return kwh_per_cycle
            .as_f64()
            .ok_or_else(|| anyhow!("kWh_per_cycle must be a float"));
    }

    if let Some(kwh_per_100cycle) = appliance.get("kWh_per_100cycle") {
        return Ok(kwh_per_100cycle
            .as_f64()
            .ok_or_else(|| anyhow!("kWh_per_100cycle must be a float"))?
            / 100.);
    }

    if let Some(kwh_per_annum) = appliance.get("kWh_per_annum") {
        // standard use is the number of cycles per annum dictated by EU standard for energy label
        let standard_use = appliance
            .get("standard_use")
            .ok_or_else(|| {
                anyhow!("Appliance '{appliance_name}' does not have a standard_use value")
            })?
            .as_f64()
            .ok_or_else(|| anyhow!("standard_use must be a float"))?;
        return Ok(kwh_per_annum
            .as_f64()
            .ok_or_else(|| anyhow!("kWh_per_annum must be a float"))?
            / standard_use);
    }

    bail!("{appliance_name} demand must be specified as one of 'kWh_per_cycle', 'kWh_per_100cycle' or 'kWh_per_annum'");
}

fn get_residual_moisture_adjustment(input: &InputForProcessing) -> anyhow::Result<f64> {
    if let Some(spin_eff_class) = input
        .appliance_with_key(CLOTHES_WASHING_APPLIANCE)?
        .and_then(|appliance| appliance.get("spin_dry_efficiency_class"))
        .and_then(|s| s.as_str())
    {
        // In accordance with section 14 of Article 2 in EU regulation 2023/2533,
        // 'eco programme' means a programme which is able to dry cotton laundry
        // from an initial moisture content of the load of 60 %
        // own to a final moisture content of the load of 0 %
        let eu_reference_res_moisture = 0.6;
        // EU Spin-drying efficiency classes and respective residual moisture contents
        let res_moisture = match spin_eff_class {
            "A" => 0.45,
            "B" => 0.54,
            "C" => 0.63,
            "D" => 0.72,
            "E" => 0.81,
            "F" => 0.9,
            "G" => 1.0,
            _ => {
                return Err(anyhow!(
                    "Spin dry efficiency class '{spin_eff_class}' is not recognised"
                ))
            }
        };

        return Ok(res_moisture / eu_reference_res_moisture);
    }

    // If spin drying efficiency of clothes washing appliance is not provided assume
    // 60% residual moisture, so no correction
    Ok(1.0)
}

/// Check (almost an assert) whether the shower flow rate is not less than the minimum allowed.
fn check_shower_flowrate(input: &InputForProcessing) -> anyhow::Result<()> {
    let min_flowrate = 8.0;

    for (name, (flowrate, allow_low_flowrate)) in input.shower_flowrates()? {
        if let (Some(flowrate), _) = (flowrate, allow_low_flowrate) {
            let allow_low_flowrate = allow_low_flowrate.unwrap_or(false);
            if !allow_low_flowrate && flowrate < min_flowrate {
                // only currently known shower name that can have a flowrate is "mixer"
                bail!(
                    "Invalid flow rate: {flowrate} litres per minute in shower with name '{name}'"
                );
            }
        }
    }
    Ok(())
}

pub(super) fn create_hot_water_use_pattern(
    input: &mut InputForProcessing,
    _tfa: f64,
    number_of_occupants: f64,
    cold_water_feed_temps: &[f64],
) -> anyhow::Result<()> {
    check_shower_flowrate(input)?;

    // temperature of mixed hot water for event
    let event_temperature_showers = 41.0;
    let event_temperature_bath = 41.0;
    let event_temperature_others = 41.0;

    let mean_feedtemp =
        cold_water_feed_temps.iter().sum::<f64>() / cold_water_feed_temps.len() as f64;
    let _mean_delta_t = HW_TEMPERATURE - mean_feedtemp;

    let _annual_hw_events: Vec<()> = vec![];
    let _annual_hw_events_energy: Vec<()> = vec![];
    let startmod = 0;

    // SAP 2012 relation
    // vol_daily_average = (25 * N_occupants) + 36

    // new relation based on Boiler Manufacturer data and EST surveys
    // reduced by 30% to account for pipework losses present in the source data
    let mut vol_hw_daily_average = 0.70 * 60.3 * number_of_occupants.powf(0.71);

    // The hot water data set only included hot water use via the central hot water system
    // Electric showers are common in the UK, sometimes in addition to a central shower.
    // It is therefore very likely more showers were taken than are recorded in our main dataset.
    // To attempt to correct for this additional shower events (and their equivalent volume)
    // need to be added for use in generating the correct list of water use events.
    // It was assumed that 30% of the homes had an additional electric shower and these were
    // used half as often as showers from the central water heating system (due to lower flow).
    // This would mean that about 15% of showers taken were missing from the data.
    // The proportion of total hot water volume due to with showers in the original sample
    // was 60.685%. Increasing this by 15%, then re-adding it to the non-shower total gives
    // 109.10%. So we need to multiply the hot water use by 1.0910 to correct for the missing showers.
    // (Note that this is only being used to generate the correct events list so does not assume
    // the dwelling being modelled actually has an electric shower, or a central shower. Allocation
    // of events to the actual showers types present in the home is done later.)
    let prop_with_elec_shower = 0.3; // 30% of homes had an additional electric shower
    let elec_shower_use_prop_of_main = 0.5; // they are used half as often as the main shower
    let correction_for_missing_elec_showers =
        1. + prop_with_elec_shower * elec_shower_use_prop_of_main; // 1.15
    let original_prop_hot_water_showers = 0.60685; // from original data set
    let uplifted_prop_hot_water_showers =
        original_prop_hot_water_showers * correction_for_missing_elec_showers;
    let elec_shower_correction_factor =
        1. - original_prop_hot_water_showers + uplifted_prop_hot_water_showers;
    vol_hw_daily_average *= elec_shower_correction_factor;

    let mut hw_event_gen = HotWaterEventGenerator::new(vol_hw_daily_average, None, None)?;
    let ref_event_list = hw_event_gen.build_annual_hw_events(startmod)?;

    let mut ref_hw_vol = 0.;

    for event in &ref_event_list {
        // NB while calibration is done by event volumes we use the event durations from the HW csv data for showers
        // so the actual hw use predicted by sap depends on shower flowrates in dwelling, but this value does not
        ref_hw_vol += event.volume;
    }

    // Add daily average hot water use to combi boiler and hot water only heat pump (HWOHP) objects,
    // if present
    // TODO (from Python) This is probably only valid if HWOHP is the only heat source for the
    // storage tank. Make this more robust/flexible in future.
    for hot_water_source in input.hot_water_source_mut()?.values_mut() {
        let source_type = hot_water_source.get("type").and_then(|t| t.as_str());
        match source_type {
            Some("StorageTank") => {
                if let Some(heat_sources) = hot_water_source
                    .get_mut("HeatSource")
                    .and_then(JsonValue::as_object_mut)
                {
                    for heat_source in heat_sources.values_mut() {
                        let heat_source = heat_source
                            .as_object_mut()
                            .ok_or_else(|| anyhow!("Heat source is not an object"))?;
                        if heat_source.get("type").and_then(|t| t.as_str())
                            == Some("HeatPump_HWOnly")
                        {
                            heat_source
                                .insert("vol_hw_daily_average".into(), json!(vol_hw_daily_average));
                        }
                    }
                }
            }
            Some("CombiBoiler") => {
                let hot_water_source = hot_water_source
                    .as_object_mut()
                    .ok_or_else(|| anyhow!("Hot water source is not an object"))?;
                hot_water_source.insert("daily_HW_usage".into(), json!(vol_hw_daily_average));
            }
            _ => {}
        };
    }

    let fhw = (365. * vol_hw_daily_average) / ref_hw_vol;

    // if part G has been complied with, apply 5% reduction to duration of Other events
    let part_g_bonus = if let Some(part_g_compliance) = input.part_g_compliance()? {
        if part_g_compliance {
            0.95
        } else {
            1.0
        }
    } else {
        bail!("Part G compliance missing from input file");
    };

    let mut hw_event_aa = reset_events_and_provide_drawoff_generator(
        number_of_occupants,
        input,
        fhw,
        event_temperature_others,
        HW_TEMPERATURE,
        cold_water_feed_temps,
        part_g_bonus,
    )?;

    // now create lists of events
    // Shower events should be evenly spread across all showers in dwelling
    // and so on for baths etc
    let mut hourly_events: Vec<Vec<HourlyHotWaterEvent>> =
        std::iter::repeat_with(Vec::new).take(8760).collect();
    for event in &ref_event_list {
        // assign HW usage events to end users and work out their durations
        // note that if there are no baths in the dwelling "bath" events are
        // assigned to showers, and vice versa
        let drawoff = if event.event_type.is_shower_type() {
            hw_event_aa.get_shower()
        } else if event.event_type.is_bath_type() {
            hw_event_aa.get_bath()
        } else {
            hw_event_aa.get_other()
        };
        let duration = drawoff.call_duration_fn(*event);

        let event_start = event.time;
        if !input.shower_name_refers_to_instant_electric(&drawoff.name) {
            // IES can overlap with anything so ignore them entirely
            // TODO (from Python) - implies 2 uses of the same IES may overlap, could check them separately
            hw_event_gen.overlap_check(
                &mut hourly_events,
                &[WaterHeatingEventType::Bath, WaterHeatingEventType::Shower],
                event_start,
                duration,
            );
            hourly_events
                .get_mut(event_start.floor() as usize)
                .unwrap()
                .push(HourlyHotWaterEvent {
                    event_type: WaterHeatingEventType::Shower,
                    start: event_start,
                    end: event_start + duration / 60.,
                });
        }

        input.add_water_heating_event(
            &drawoff.event_type,
            &drawoff.name,
            json!({
                "start": event_start,
                "duration": Some(duration),
                "volume": if event.event_type.is_bath_type() {
                    // if the end user the event is being assigned to has a defined flowrate
                    // we are able to supply a volume
                    input
                        .flowrate_for_bath_field(&drawoff.name)?
                        .map(|flowrate| duration * flowrate)
                } else {
                    None
                },
                "temperature": if event.event_type.is_shower_type() {
                    event_temperature_showers
                } else if event.event_type.is_bath_type() {
                    event_temperature_bath
                } else {
                    event_temperature_others
                },
            }),
        )?;
    }

    Ok(())
}

fn window_treatment(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let simtime = simtime();
    let extcond = create_external_conditions(input.external_conditions()?, &simtime.iter())?;
    let mut curtain_opening_sched_manual: Vec<Option<bool>> = Default::default();
    let mut curtain_opening_sched_auto: Vec<bool> = Default::default();
    let mut blinds_closing_irrad_manual: Vec<Option<f64>> = Default::default();
    let mut blinds_opening_irrad_manual: Vec<Option<f64>> = Default::default();

    for t_it in simtime.iter() {
        let hour_of_day = t_it.hour_of_day() as usize;
        // TODO (from Python) Are these waking hours correct? Check consistency with other parts of calculation
        let waking_hours = (OCCUPANT_WAKING_HR..OCCUPANT_SLEEPING_HR).contains(&hour_of_day);
        let sun_above_horizon = extcond.sun_above_horizon(t_it);

        curtain_opening_sched_manual.push(if waking_hours && sun_above_horizon {
            Some(true) // Open during waking hours after sunrise
        } else if waking_hours && !sun_above_horizon {
            Some(false) // Close during waking hours after sunset
        } else {
            None // Do not adjust outside waking hours
        });
        curtain_opening_sched_auto.push(sun_above_horizon);
        blinds_closing_irrad_manual.push(if waking_hours { Some(300.) } else { None });
        blinds_opening_irrad_manual.push(if waking_hours { Some(200.) } else { None });
    }

    input.add_control(
        "_curtains_open_manual",
        json!({
            "type": "OnOffTimeControl",
            "allow_null": true,
            "start_day": 0,
            "time_series_step": SIMTIME_STEP,
            "schedule": {
                "main": curtain_opening_sched_manual,
            }
        }),
    )?;

    input.add_control(
        "_curtains_open_auto",
        json!({
            "type": "OnOffTimeControl",
            "start_day": 0,
            "time_series_step": SIMTIME_STEP,
            "schedule": {
                "main": curtain_opening_sched_auto,
            }
        }),
    )?;

    input.add_control(
        "_blinds_closing_irrad_manual",
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": SIMTIME_STEP,
            "schedule": {
                "main": blinds_closing_irrad_manual,
            }
        }),
    )?;

    input.add_control(
        "_blinds_closing_irrad_auto",
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 1.,
            "schedule": {
                "main": [{"repeat": SIMTIME_END as usize, "value": 200.}],
            }
        }),
    )?;

    input.add_control(
        "_blinds_opening_irrad_manual",
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": SIMTIME_STEP,
            "schedule": {
                "main": blinds_opening_irrad_manual,
            }
        }),
    )?;

    input.add_control(
        "_blinds_opening_irrad_auto",
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 1.,
            "schedule": {
                "main": [{"repeat": SIMTIME_END as usize, "value": 200.}],
            }
        }),
    )?;

    let transparent_building_elements = input.all_transparent_building_elements_mut()?;

    for mut building_element in transparent_building_elements
        .into_iter()
        .map(TransparentBuildingElementJsonValue)
    {
        for treatment in building_element.treatment().iter_mut().flatten() {
            treatment.insert("is_open".into(), json!(false));
            if let Some(treatment_type) = treatment
                .get("type")
                .and_then(|treatment_type| treatment_type.as_str())
            {
                let treatment_controls_are_manual = treatment
                    .get("WindowTreatmentControl")
                    .and_then(|window_control| window_control.as_str())
                    .is_some_and(|window_control_type| window_control_type.starts_with("manual"));
                match treatment_type {
                    "curtains" => {
                        if treatment_controls_are_manual {
                            treatment.insert("Control_open".into(), json!("_curtains_open_manual"));
                        } else {
                            treatment.insert("Control_open".into(), json!("_curtains_open_auto"));
                        }
                    }
                    "blinds" => {
                        if treatment_controls_are_manual {
                            // manual control - Table B.24 in BS EN ISO 52016-1:2017.
                            treatment.insert(
                                "Control_closing_irrad".into(),
                                json!("_blinds_closing_irrad_manual"),
                            );
                            treatment.insert(
                                "Control_opening_irrad".into(),
                                json!("_blinds_opening_irrad_manual"),
                            );
                        } else {
                            // automatic control - Table B.24 in BS EN ISO 52016-1:2017.
                            treatment.insert(
                                "Control_closing_irrad".into(),
                                json!("_blinds_closing_irrad_auto"),
                            );
                            treatment.insert(
                                "Control_opening_irrad".into(),
                                json!("_blinds_opening_irrad_auto"),
                            );
                            treatment.insert("opening_delay_hrs".into(), json!(2));
                        }
                    }
                    _ => {
                        // do nothing
                    }
                }
            }
        }
    }

    Ok(())
}

fn create_heating(input: &mut InputForProcessing) -> anyhow::Result<()> {
    for heating_system in input.space_heat_systems_mut()?.values_mut() {
        if heating_system
            .get("type")
            .and_then(JsonValue::as_str)
            .is_some_and(|type_str| type_str == "InstantElecHeater")
        {
            if let Some(convective_type) = heating_system
                .get("convective_type")
                .and_then(JsonValue::as_str)
            {
                let frac_convective_value = match convective_type {
                    "Air heating (convectors, fan coils etc.)" => 0.95,
                    "Free heating surface (radiators, radiant panels etc.)" => 0.70,
                    "Floor heating, low temperature radiant tube heaters, luminous heaters, wood stoves" => 0.50,
                    "Wall heating, radiant ceiling panels, accumulation stoves" => 0.35,
                    "Ceiling heating, radiant ceiling electric heating" => 0.20,
                    _ => bail!("Unknown convective type encountered: {convective_type}"),
                };
                let heating_system = heating_system
                    .as_object_mut()
                    .ok_or_else(|| anyhow!("Expected object"))?;
                heating_system.insert("frac_convective".into(), json!(frac_convective_value));
                heating_system.remove("convective_type");
            }
        }
    }

    if let Some(wet_heat_sources) = input.optional_root_object_mut("WetHeatSource")? {
        for heat_source in wet_heat_sources.values_mut() {
            if heat_source
                .get("type")
                .and_then(JsonValue::as_str)
                .is_some_and(|type_str| type_str == "HeatBattery")
            {
                if let Some(heat_source) = heat_source.as_object_mut() {
                    heat_source.insert("heat_battery_location".into(), json!("internal"));
                }
            }
        }
    }

    Ok(())
}

fn create_infiltration_ventilation(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let test_pressure_node = input.infiltration_ventilation_node_mut()?.get_mut("Leaks").and_then(JsonValue::as_object_mut).and_then(|node| node.get_mut("test_pressure")).ok_or_else(|| anyhow!("The `test_pressure` field for infiltration ventilation leaks could not be found when expected."))?;
    let test_pressure_value = match test_pressure_node.as_str() {
        Some("Standard") => 50,
        Some("Pulse test only") => 4,
        _ => bail!("The `test_pressure` field for infiltration ventilation leaks must be either `Standard` or `Pulse test only`."),
    };
    *test_pressure_node = json!(test_pressure_value);

    Ok(())
}

static COLOUR_TO_SOLAR_ABSORPTION_MAP: LazyLock<IndexMap<&'static str, f64>> =
    LazyLock::new(|| [("Light", 0.3), ("Intermediate", 0.6), ("Dark", 0.9)].into());
static AREAL_HEAT_MAP: LazyLock<IndexMap<&'static str, usize>> = LazyLock::new(|| {
    [
        ("Very light", 50000),
        ("Light", 75000),
        ("Medium", 110000),
        ("Heavy", 175000),
        ("Very heavy", 250000),
    ]
    .into()
});
static MASS_DISTRIBUTION_MAP: LazyLock<IndexMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        [
            ("I: Mass concentrated at internal side", "I"),
            ("E: Mass concentrated at external side", "E"),
            ("IE: Mass divided over internal and external side", "IE"),
            ("D: Mass equally distributed", "D"),
            ("M: Mass concentrated inside", "M"),
        ]
        .into()
    });

pub(crate) fn create_thermal_penetration(input: &mut InputForProcessing) -> anyhow::Result<()> {
    for building_element in input.all_building_elements_mut()? {
        let element_type = building_element
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("Building element type not found"))?
            .to_owned();
        if element_type == "BuildingElementOpaque" {
            let solar_absorption_value = *COLOUR_TO_SOLAR_ABSORPTION_MAP
                .get(
                    building_element
                        .get("colour")
                        .and_then(|c| c.as_str())
                        .ok_or_else(|| anyhow!("Building element colour was not a string."))?,
                )
                .ok_or_else(|| {
                    anyhow!(
                        "Unrecognised building element colour '{}' passed.",
                        building_element["colour"]
                    )
                })?;
            building_element.insert(
                "solar_absorption_coeff".into(),
                json!(solar_absorption_value),
            );
            building_element.remove("colour");
        }
        if [
            "BuildingElementOpaque",
            "BuildingElementGround",
            "BuildingElementAdjacentConditionedSpace",
            "BuildingElementAdjacentUnconditionedSpace_Simple",
            "BuildingElementPartyWall",
        ]
        .contains(&element_type.as_str())
        {
            let areal_heat_value = *AREAL_HEAT_MAP
                .get(
                    building_element
                        .get("areal_heat_capacity")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            anyhow!("Building element areal heat capacity was not a string.")
                        })?,
                )
                .ok_or_else(|| {
                    anyhow!(
                        "Building element areal heat capacity had unexpected value '{}'.",
                        building_element["areal_heat_capacity"]
                    )
                })?;
            building_element["areal_heat_capacity"] = json!(areal_heat_value);
            let mass_distribution_value = MASS_DISTRIBUTION_MAP
                .get(
                    building_element
                        .get("mass_distribution_class")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            anyhow!(
                                "Building element mass distribution class value was not a string."
                            )
                        })?,
                )
                .ok_or_else(|| {
                    anyhow!(
                        "Building element mass distribution value '{}' was not recognised.",
                        building_element["mass_distribution_class"]
                    )
                })?;
            building_element["mass_distribution_class"] = json!(mass_distribution_value);
        }
    }

    Ok(())
}

pub(super) fn create_window_opening_schedule(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let window_opening_setpoint = 22.0;

    input.add_control(
        "_window_opening_adjust",
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 1.0,
            "schedule": {
                "main": [{"repeat": SIMTIME_END as usize, "value": window_opening_setpoint}],
            }
        }),
    )?;
    input.set_window_adjust_control_for_infiltration_ventilation("_window_opening_adjust")?;

    input.add_control(
        "_window_opening_openablealways",
        json!({
            "type": "OnOffTimeControl",
            "start_day": 0,
            "time_series_step": 1.0,
            "schedule": {
                "main": [{"repeat": SIMTIME_END as usize, "value": true}]
            }
        }),
    )?;

    input.add_control(
        "_window_opening_closedsleeping",
        json!({
            "type": "OnOffTimeControl",
            "start_day": 0,
            "time_series_step": 1.0,
            "schedule": {
                "main": [{"repeat": 365, "value": "day"}],
                "day": [
                    {"repeat": OCCUPANT_WAKING_HR, "value": false},
                    {"repeat": OCCUPANT_SLEEPING_HR - OCCUPANT_WAKING_HR, "value": true},
                    {"repeat": 24 - OCCUPANT_SLEEPING_HR, "value": false},
                ]
            }
        }),
    )?;

    let noise_nuisance = input.infiltration_ventilation_is_noise_nuisance();

    for mut transparent_building_element in input
        .all_transparent_building_elements_mut()?
        .into_iter()
        .map(TransparentBuildingElementJsonValue)
    {
        let element_is_security_risk = transparent_building_element.is_security_risk();
        transparent_building_element.set_window_openable_control(
            if noise_nuisance || element_is_security_risk {
                "_window_opening_closedsleeping"
            } else {
                "_window_opening_openablealways"
            },
        );
    }

    Ok(())
}

/// Set min and max vent opening thresholds
fn create_vent_opening_schedule(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let vent_adjust_min_ach = 1.9;
    let vent_adjust_max_ach = 2.;

    input.add_control(
        "_vent_adjust_min_ach",
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 1.0,
            "schedule": {
                "main": [{"repeat": (SIMTIME_END - SIMTIME_START) as usize, "value": vent_adjust_min_ach}],
            }
        }),
    )?;
    input.set_vent_adjust_min_control_for_infiltration_ventilation("_vent_adjust_min_ach")?;

    input.add_control(
        "_vent_adjust_max_ach",
        json!({
            "type": "SetpointTimeControl",
            "start_day": 0,
            "time_series_step": 1.0,
            "schedule": {
                "main": [{"repeat": (SIMTIME_END - SIMTIME_START) as usize, "value": vent_adjust_max_ach}],
            }
        }),
    )?;
    input.set_vent_adjust_max_control_for_infiltration_ventilation("_vent_adjust_max_ach")?;

    Ok(())
}

fn calc_sfp_mech_vent(input: &mut InputForProcessing) -> anyhow::Result<()> {
    for mech_vents_data in input.mechanical_ventilations_for_processing()? {
        if !mech_vents_data.contains_key("SFP") {
            let measured_fan_power = mech_vents_data.get("measured_fan_power").and_then(JsonValue::as_f64).ok_or_else(|| anyhow!("Mechanical ventilation data was missing a numeric 'measured_fan_power' field"))?; // in W
            let measured_air_flow_rate = mech_vents_data.get("measured_air_flow_rate").and_then(JsonValue::as_f64).ok_or_else(|| anyhow!("Mechanical ventilation data was missing a numeric 'measured_air_flow_rate' field"))?;
            // in l/s
            // Specific fan power is total measured electrical power in Watts divided
            // by air flow rate
            let measured_sfp = measured_fan_power / measured_air_flow_rate;
            mech_vents_data.insert("SFP".into(), json!(measured_sfp));
        }
    }

    Ok(())
}

fn create_cooling(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let zone_keys = input.zone_keys()?;
    for zone_key in &zone_keys {
        if input.zone_has_space_cool_system(zone_key)? {
            for space_cool_system in input.space_cool_system_for_zone(zone_key)?.iter() {
                let ctrl_name = format!("Cooling_{space_cool_system}");

                input.set_control_string_for_space_cool_system(space_cool_system, &ctrl_name)?;

                let mut control = json!({
                    "type": "SetpointTimeControl",
                    "start_day" : 0,
                    "time_series_step":0.5,
                    "schedule": {
                        "main": [{"repeat": 53, "value": "week"}],
                        "week": [{"repeat": 5, "value": "weekday"},
                                    {"repeat": 2, "value": "weekend"}],
                        "weekday": COOLING_SUBSCHEDULE_WEEKDAY.to_vec(),
                        "weekend": COOLING_SUBSCHEDULE_WEEKEND.to_vec(),
                    }
                });

                let control_object = control.as_object_mut().unwrap();
                if let Some(temp_setback) =
                    input.temperature_setback_for_space_cool_system(space_cool_system)?
                {
                    control_object.insert("setpoint_max".to_string(), temp_setback.into());
                }
                if let Some(advanced_start) =
                    input.advanced_start_for_space_cool_system(space_cool_system)?
                {
                    control_object.insert("advanced_start".to_string(), advanced_start.into());
                }
                input.add_control(&ctrl_name, json!(control_object))?;
            }
        }
    }

    Ok(())
}

const COOLING_SETPOINT: f64 = 24.0;

// 07:00-09:30 and then 18:30-22:00
const COOLING_SUBSCHEDULE_WEEKDAY: [Option<f64>; 48] = [
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    None,
    None,
    None,
    None,
];

// 08:30-22:30
const COOLING_SUBSCHEDULE_WEEKEND: [Option<f64>; 48] = [
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    Some(COOLING_SETPOINT),
    None,
    None,
    None,
];

pub(super) fn create_cold_water_feed_temps(
    input: &mut InputForProcessing,
) -> anyhow::Result<Vec<f64>> {
    // 24-hour average feed temperature (degrees Celsius) per month m. SAP 10.2 Table J1
    let t24m_header_tank = [
        11.1, 11.3, 12.3, 14.5, 16.2, 18.8, 21.3, 19.3, 18.7, 16.2, 13.2, 11.2,
    ];
    let t24m_mains = [
        8.0, 8.2, 9.3, 12.7, 14.6, 16.7, 18.4, 17.6, 16.6, 14.3, 11.1, 8.5,
    ];
    // typical fall in feed temp from midnight to 6am
    let delta = 1.5;

    let (t24m, feed_type) = if input.cold_water_source_has_header_tank()? {
        (t24m_header_tank, "header tank")
    } else {
        (t24m_mains, "mains water")
    };

    let mut cold_feed_schedule_m: Vec<Vec<f64>> = Vec::with_capacity(12 * 24);

    for t in t24m {
        // typical cold feed temp between 3pm and midnight
        let t_evening_m = t + (delta * 15. / 48.);

        // variation throughout the day
        cold_feed_schedule_m.push(
            (0..6)
                .map(|t| t_evening_m - delta * t as f64 / 6.)
                .chain((6..15).map(|t| t_evening_m - (15 - t) as f64 * delta / 9.))
                .chain((15..24).map(|_| t_evening_m))
                .collect(),
        );
    }

    let output_feed_temp = repeat_n(&cold_feed_schedule_m[0], 31)
        .flatten()
        .chain(repeat_n(&cold_feed_schedule_m[1], 28).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[2], 31).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[3], 30).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[4], 31).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[5], 30).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[6], 31).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[7], 31).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[8], 30).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[9], 31).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[10], 30).flatten())
        .chain(repeat_n(&cold_feed_schedule_m[11], 31).flatten())
        .cloned()
        .collect::<Vec<_>>();

    input.set_cold_water_source_by_key(
        feed_type,
        json!({
            "start_day": 0,
            "time_series_step": 1.,
            "temperatures": output_feed_temp.to_vec(),
        }),
    )?;

    Ok(output_feed_temp)
}

/// Add an area property to each zone
/// Assumes the presence of livingroom_area and restofdwelling_area properties in
/// each project_dict.Zone[<zone_name>] (as required by the FHS schema),
/// and sets/creates project_dict.Zone[<zone_name>]["area"] (as required by the
/// hem_core schema) with a value equal to the sum of those two component areas.
///
/// Args:
/// * `project_dict` (dict) - The main project dictionary where results are stored.
///
/// Effects:
/// * Modifies the project_dict in-place by setting the zone area property.
pub(super) fn create_zone_area(input: &mut InputForProcessing) -> anyhow::Result<()> {
    for zone in input.zone_keys()? {
        let living_room_area = input.living_room_area_for_zone(&zone)?;
        let rest_of_dwelling_area = input.rest_of_dwelling_area_for_zone(&zone)?;
        input.set_area_for_zone(&zone, living_room_area + rest_of_dwelling_area)?;
    }
    Ok(())
}

fn daylight_factor(input: &InputForProcessing, total_floor_area: f64) -> anyhow::Result<Vec<f64>> {
    let mut total_area = vec![0.; simtime().total_steps()];
    let data: Vec<Vec<f64>> = input
        .all_building_elements()?
        .values()
        .filter(|el| el.get("type").and_then(|t| t.as_str()) == Some("BuildingElementTransparent"))
        .map(|el| {
            fn get_field_as_f64(value: &JsonValue, field: &str) -> anyhow::Result<f64> {
                let error_message = format!(
                    "Field '{}' missing or invalid for transparent building element",
                    field
                );
                value
                    .get(field)
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| anyhow!(error_message))
            }

            let ff = get_field_as_f64(el, "frame_area_fraction")?;
            let g_value = get_field_as_f64(el, "g_value")?;
            let width = get_field_as_f64(el, "width")?;
            let height = get_field_as_f64(el, "height")?;
            let base_height = get_field_as_f64(el, "base_height")?;
            let orientation = get_field_as_f64(el, "orientation360")?;
            let shading = el
                .get("shading")
                .ok_or_else(|| anyhow!("Shading field missing for transparent building element"))?;

            let shading: Vec<WindowShadingObject> = serde_json::from_value(json!(shading))?;

            let w_area = width * height;

            // retrieve half-hourly shading factor
            let direct_result = shading_factor(
                input,
                base_height,
                height,
                width,
                Orientation360::from(orientation),
                &shading,
            )?;

            let area = 0.9 * w_area * (1. - ff) * g_value;
            Ok::<Vec<f64>, anyhow::Error>(
                direct_result.iter().map(|factor| factor * area).collect(),
            )
        })
        .collect::<anyhow::Result<Vec<_>, _>>()?;

    for idx in data {
        for (t, gl) in idx.into_iter().enumerate() {
            total_area[t] += gl;
        }
    }

    // calculate Gl for each half hourly timestep
    Ok((0..total_area.len())
        .map(|i| {
            let gl = total_area[i] / total_floor_area;

            if gl > 0.095 {
                0.96
            } else {
                52.2 * gl.powi(2) - 9.94 * gl + 1.433
            }
        })
        .collect())
}

fn shading_factor(
    input: &InputForProcessing,
    base_height: f64,
    height: f64,
    width: f64,
    orientation: Orientation360,
    shading: &[WindowShadingObject],
) -> anyhow::Result<Vec<f64>> {
    // there is code in the upstream Python to convert orientations from -180 to +180 (anticlockwise) to 0-360 (clockwise)
    // but the Rust input code has already implicitly performed this conversion on the way in, so we don't need to do it here

    let time = simtime();

    let input_external_conditions = input.external_conditions()?;

    let dir_beam_conversion = input_external_conditions
        .direct_beam_conversion_needed()
        .is_some_and(|x| x);

    let conditions = ExternalConditions::new(
        &time.iter(),
        input_external_conditions
            .air_temperatures()
            .as_ref()
            .ok_or_else(|| anyhow!("Air temps were expected in input and not provided."))?
            .to_vec(),
        input_external_conditions
            .wind_speeds()
            .as_ref()
            .ok_or_else(|| anyhow!("Wind speeds were expected in input and not provided."))?
            .to_vec(),
        input_external_conditions
            .wind_directions()
            .as_ref()
            .ok_or_else(|| anyhow!("Wind directions were expected in input and not provided."))?
            .to_vec(),
        input_external_conditions
            .diffuse_horizontal_radiation()
            .as_ref()
            .ok_or_else(|| {
                anyhow!("Diffuse horizontal radiations were expected in input and not provided.")
            })?
            .to_vec(),
        input_external_conditions
            .direct_beam_radiation()
            .as_ref()
            .ok_or_else(|| {
                anyhow!("Direct beam radiations were expected in input and not provided.")
            })?
            .to_vec(),
        input_external_conditions
            .solar_reflectivity_of_ground()
            .as_ref()
            .ok_or_else(|| {
                anyhow!("Solar reflectivity of ground was expected in input and not provided.")
            })?
            .to_vec(),
        input_external_conditions
            .latitude()
            .ok_or_else(|| anyhow!("Latitude was expected in input and not provided."))?,
        input_external_conditions
            .longitude()
            .ok_or_else(|| anyhow!("Longitude was expected in input and not provided."))?,
        0,
        0,
        Some(365),
        1.,
        None,
        None,
        false,
        dir_beam_conversion,
        input_external_conditions
            .shading_segments()
            .map(|x| x.to_vec()),
    );

    time.iter()
        .map(|t_it| {
            conditions.direct_shading_reduction_factor(
                base_height,
                height,
                width,
                orientation,
                Some(shading),
                t_it,
            )
        })
        .collect()
}

fn top_up_lighting(
    input: &InputForProcessing,
    l_req: f64,
    total_capacity: f64,
) -> anyhow::Result<f64> {
    if !input.all_zones_have_bulbs()? {
        bail!("At least one zone has lighting that does not have bulbs defined.");
    }

    let tfa = calc_tfa(input)?;
    let capacity_ref = 330. * tfa;

    let l_prov = l_req * (total_capacity / capacity_ref);

    let l_topup = if l_prov < (l_req / 3.) {
        (l_req / 3.) - l_prov
    } else {
        0.
    };

    Ok(l_topup)
}

fn create_hot_water_distribution(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let number_of_hot_tapped_rooms = input.number_of_hot_tapped_rooms()?;
    let non_kitchen_tapped_rooms = number_of_hot_tapped_rooms - 1;
    let number_of_storeys = input.storeys_in_dwelling()? as f64;
    let building_length = input.building_length()?;
    let building_width = input.building_width()?;
    // Calculate habitable building height
    let habitable_building_height = habitable_building_height(input)?;
    // Pipe calculations
    let lateral_pipe_factor = 0.0625;
    let vertical_pipe_factor = 0.038;
    let branch_circuit_factor = 0.0625;
    let reduction_factor = 2.;
    let main_distribution_pipe_length =
        building_length + (lateral_pipe_factor * building_length * building_width);
    let main_shaft_pipe_length =
        building_length * building_width * habitable_building_height * vertical_pipe_factor;
    let small_vertical_pipe_length = main_shaft_pipe_length * non_kitchen_tapped_rooms as f64;
    let branching_pipe_length =
        building_length * building_width * number_of_storeys * branch_circuit_factor;

    let small_pipe_length = (branching_pipe_length + small_vertical_pipe_length) / reduction_factor;
    let large_pipe_length =
        main_distribution_pipe_length * non_kitchen_tapped_rooms as f64 / reduction_factor;
    let distribution = json!([
        {"internal_diameter_mm": 13, "length": 0.1_f64.max((small_pipe_length * 100.).round_ties_even() / 100.), "location": "internal"},
        {"internal_diameter_mm": 20, "length": 0.1_f64.max((large_pipe_length * 100.).round_ties_even() / 100.), "location": "internal"},
    ]);
    input.set_water_distribution(distribution)?;
    Ok(())
}

/// Ensures that input "HotWaterDemand" exists and contains required sub-keys.
fn create_hot_water_demand(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let hot_water_demand = input.root_object_entry_mut("HotWaterDemand")?;

    hot_water_demand.entry("Shower").or_insert(json!({}));
    hot_water_demand.entry("Bath").or_insert(json!({}));
    hot_water_demand.entry("Other").or_insert(json!({}));

    Ok(())
}

/// The EnergySupply of a heat network is exclusively allowed to be a custom object defining
/// a custom fuel. In this case we need to move the custom object into EnergySupply and
/// reference it for the heat network. We extract out the custom energy factors to be used later.
fn create_custom_energy_supply_factors(
    input: &mut InputForProcessing,
) -> anyhow::Result<IndexMap<Arc<str>, CustomEnergySourceFactor>> {
    let mut custom_energy_supply_factors = IndexMap::new();

    let heat_source_wet_keys = input
        .heat_source_wet()?
        .keys()
        .cloned()
        .collect::<Vec<String>>();

    for heat_source_wet_key in heat_source_wet_keys {
        let is_heat_network = {
            input
                .heat_source_wet_by_key(&heat_source_wet_key)?
                .get("is_heat_network")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        };

        if let Some((Some(name), Some(factor), Some(is_export_capable))) = input
            .heat_source_wet_by_key(&heat_source_wet_key)?
            .get("EnergySupply")
            .and_then(|v| v.as_object())
            .map(|v| {
                (
                    v.get("name").and_then(|v| v.as_str()).map(String::from),
                    v.get("factor").and_then(|v| v.as_object()),
                    v.get("is_export_capable").and_then(|v| v.as_bool()),
                )
            })
        {
            if is_heat_network {
                // Extract custom energy factor
                custom_energy_supply_factors.insert(
                    Arc::<str>::from(name.to_string()),
                    serde_json::from_value(json!(factor))?,
                );
                // Create new top level EnergySupply
                if input.energy_supplies_contain_key(&name)? {
                    bail!("An EnergySupply named '{name}' already exists. Unable to add a custom EnergySupply for HeatSourceWet '{heat_source_wet_key}' with the same name.");
                }
                input.add_energy_supply_for_key(
                    &name,
                    json!({
                        "fuel": "custom",
                        "is_export_capable": is_export_capable,
                    }),
                )?;
                input
                    .heat_source_wet_by_key_mut(&heat_source_wet_key)?
                    .insert("EnergySupply".into(), json!(name));
            }
        }

        // Process custom EnergySupply_heat_network dicts
        if let Some((Some(name), Some(factor), Some(is_export_capable))) = input
            .heat_source_wet_by_key(&heat_source_wet_key)?
            .get("EnergySupply_heat_network")
            .and_then(|v| v.as_object())
            .map(|v| {
                (
                    v.get("name").and_then(|v| v.as_str()).map(String::from),
                    v.get("factor").and_then(|v| v.as_object()),
                    v.get("is_export_capable").and_then(|v| v.as_bool()),
                )
            })
        {
            // Extract custom energy factor
            custom_energy_supply_factors.insert(
                Arc::<str>::from(name.to_string()),
                serde_json::from_value(json!(factor))?,
            );
            // Create new top level EnergySupply
            if input.energy_supplies_contain_key(&name)? {
                bail!("An EnergySupply named '{name}' already exists. Unable to add a custom EnergySupply_heat_network for HeatSourceWet '{heat_source_wet_key}' with the same name.");
            }
            input.add_energy_supply_for_key(
                &name,
                json!({
                    "fuel": "custom",
                    "is_export_capable": is_export_capable,
                }),
            )?;
            // Replace heat source's EnergySupply_heat_network dict with name (str) reference
            // to new top level EnergySupply
            input
                .heat_source_wet_by_key_mut(&heat_source_wet_key)?
                .insert("EnergySupply_heat_network".into(), json!(name));
        }
    }

    Ok(custom_energy_supply_factors)
}

pub(crate) fn remove_fhs_only_inputs(input: &mut InputForProcessing) -> anyhow::Result<()> {
    // detail of removal of FHS fields is delegated to input here
    input.remove_fhs_only_fields()?;

    Ok(())
}

#[derive(Clone, Copy)]
pub struct HourlyHotWaterEvent {
    pub event_type: WaterHeatingEventType,
    pub start: f64,
    pub end: f64,
}

const AVERAGE_MONTHLY_LIGHTING_HALF_HOUR_PROFILES: [[f64; 48]; 12] = [
    [
        0.029235831,
        0.02170637,
        0.016683155,
        0.013732757,
        0.011874713,
        0.010023118,
        0.008837131,
        0.007993816,
        0.007544302,
        0.007057335,
        0.007305208,
        0.007595198,
        0.009170401,
        0.013592425,
        0.024221707,
        0.034538234,
        0.035759809,
        0.02561524,
        0.019538678,
        0.017856399,
        0.016146846,
        0.014341097,
        0.013408345,
        0.013240894,
        0.013252628,
        0.013314013,
        0.013417126,
        0.01429735,
        0.014254224,
        0.014902582,
        0.017289786,
        0.023494947,
        0.035462982,
        0.050550653,
        0.065124006,
        0.072629223,
        0.073631053,
        0.074451912,
        0.074003097,
        0.073190397,
        0.071169797,
        0.069983033,
        0.06890179,
        0.066130187,
        0.062654436,
        0.056634675,
        0.047539646,
        0.037801233,
    ],
    [
        0.026270349,
        0.01864863,
        0.014605535,
        0.01133541,
        0.009557625,
        0.008620514,
        0.007385915,
        0.00674999,
        0.006144089,
        0.005812534,
        0.005834644,
        0.006389013,
        0.007680219,
        0.013106226,
        0.021999709,
        0.027144574,
        0.02507541,
        0.0179487,
        0.014855879,
        0.012930469,
        0.011690622,
        0.010230198,
        0.00994897,
        0.009668602,
        0.00969183,
        0.010174279,
        0.011264866,
        0.011500069,
        0.011588248,
        0.011285427,
        0.012248949,
        0.014420402,
        0.01932017,
        0.027098032,
        0.044955369,
        0.062118024,
        0.072183735,
        0.075100799,
        0.075170654,
        0.072433133,
        0.070588417,
        0.069756433,
        0.068356831,
        0.06656098,
        0.06324827,
        0.055573729,
        0.045490296,
        0.035742204,
    ],
    [
        0.02538112,
        0.018177936,
        0.012838313,
        0.00961673,
        0.007914015,
        0.006844738,
        0.00611386,
        0.005458354,
        0.00508359,
        0.004864933,
        0.004817922,
        0.005375289,
        0.006804643,
        0.009702514,
        0.013148583,
        0.013569968,
        0.01293754,
        0.009183378,
        0.007893734,
        0.00666975,
        0.006673791,
        0.006235776,
        0.006096299,
        0.006250229,
        0.006018285,
        0.00670324,
        0.006705105,
        0.006701531,
        0.006893458,
        0.006440525,
        0.006447363,
        0.007359989,
        0.009510975,
        0.011406472,
        0.017428875,
        0.026635564,
        0.042951415,
        0.057993474,
        0.066065305,
        0.067668248,
        0.067593187,
        0.067506237,
        0.065543759,
        0.063020652,
        0.06004127,
        0.052838397,
        0.043077683,
        0.033689246,
    ],
    [
        0.029044978,
        0.020558675,
        0.014440871,
        0.010798435,
        0.008612364,
        0.007330799,
        0.006848797,
        0.006406058,
        0.00602619,
        0.005718987,
        0.005804901,
        0.006746423,
        0.007160898,
        0.008643678,
        0.010489867,
        0.011675722,
        0.011633729,
        0.008939881,
        0.007346857,
        0.007177037,
        0.007113926,
        0.007536109,
        0.007443049,
        0.006922747,
        0.00685514,
        0.006721853,
        0.006695838,
        0.005746367,
        0.005945173,
        0.005250153,
        0.005665752,
        0.006481695,
        0.006585193,
        0.00751989,
        0.009038481,
        0.009984259,
        0.011695555,
        0.014495872,
        0.018177089,
        0.027110627,
        0.042244993,
        0.056861545,
        0.064008071,
        0.062680016,
        0.060886258,
        0.055751568,
        0.048310205,
        0.038721632,
    ],
    [
        0.023835444,
        0.016876637,
        0.012178456,
        0.009349274,
        0.007659691,
        0.006332517,
        0.005611274,
        0.005650048,
        0.005502101,
        0.005168442,
        0.005128425,
        0.005395259,
        0.004998272,
        0.005229362,
        0.006775116,
        0.007912694,
        0.008514274,
        0.006961449,
        0.00630672,
        0.00620858,
        0.005797218,
        0.005397357,
        0.006006318,
        0.005593869,
        0.005241095,
        0.005212189,
        0.00515531,
        0.004906504,
        0.004757624,
        0.004722969,
        0.004975738,
        0.005211879,
        0.005684004,
        0.006331507,
        0.007031149,
        0.008034144,
        0.008731998,
        0.010738922,
        0.013170262,
        0.016638631,
        0.021708313,
        0.0303703,
        0.043713685,
        0.051876584,
        0.054591464,
        0.05074126,
        0.043109775,
        0.033925231,
    ],
    [
        0.023960632,
        0.016910619,
        0.012253193,
        0.009539031,
        0.007685214,
        0.006311553,
        0.00556675,
        0.005140391,
        0.004604673,
        0.004352551,
        0.004156956,
        0.004098101,
        0.00388452,
        0.00433039,
        0.005658606,
        0.006828804,
        0.007253075,
        0.005872749,
        0.004923197,
        0.004521087,
        0.004454765,
        0.004304616,
        0.004466648,
        0.004178716,
        0.004186183,
        0.003934784,
        0.004014114,
        0.003773073,
        0.003469885,
        0.003708517,
        0.003801095,
        0.004367245,
        0.004558263,
        0.005596378,
        0.005862632,
        0.006068665,
        0.006445161,
        0.007402661,
        0.007880006,
        0.009723385,
        0.012243076,
        0.016280074,
        0.023909324,
        0.03586776,
        0.046595858,
        0.047521241,
        0.041417407,
        0.03322265,
    ],
    [
        0.024387138,
        0.017950032,
        0.01339296,
        0.010486231,
        0.008634325,
        0.00752814,
        0.006562675,
        0.006180296,
        0.00566116,
        0.005092682,
        0.004741384,
        0.004680853,
        0.00479228,
        0.004921812,
        0.005950605,
        0.007010479,
        0.007057257,
        0.005651136,
        0.004813649,
        0.00454666,
        0.004121156,
        0.003793481,
        0.004122788,
        0.004107635,
        0.004363668,
        0.004310674,
        0.004122943,
        0.004014391,
        0.004009496,
        0.003805058,
        0.004133355,
        0.004188447,
        0.005268291,
        0.005964825,
        0.005774607,
        0.006292344,
        0.006813734,
        0.007634982,
        0.008723529,
        0.009855823,
        0.012318322,
        0.017097237,
        0.026780014,
        0.037823534,
        0.046797578,
        0.045940354,
        0.039472789,
        0.033058217,
    ],
    [
        0.023920296,
        0.01690733,
        0.012917415,
        0.010191735,
        0.008787867,
        0.007681138,
        0.006600128,
        0.006043227,
        0.005963814,
        0.005885256,
        0.006164212,
        0.005876554,
        0.005432168,
        0.00580157,
        0.00641092,
        0.007280576,
        0.00811752,
        0.007006283,
        0.006505718,
        0.005917892,
        0.005420978,
        0.005527121,
        0.005317478,
        0.004793601,
        0.004577663,
        0.004958332,
        0.005159584,
        0.004925386,
        0.005192686,
        0.0054453,
        0.005400465,
        0.005331386,
        0.005994507,
        0.006370203,
        0.006800758,
        0.007947816,
        0.009005592,
        0.010608225,
        0.012905449,
        0.015976909,
        0.024610768,
        0.036414926,
        0.04680022,
        0.050678553,
        0.051188831,
        0.046725936,
        0.03998602,
        0.032496965,
    ],
    [
        0.022221313,
        0.016428778,
        0.01266253,
        0.010569518,
        0.008926713,
        0.007929788,
        0.007134802,
        0.006773883,
        0.006485147,
        0.006766094,
        0.007202971,
        0.007480145,
        0.008460127,
        0.011414527,
        0.014342431,
        0.01448993,
        0.012040415,
        0.008520428,
        0.0077578,
        0.006421555,
        0.005889369,
        0.005915144,
        0.006229011,
        0.005425193,
        0.005094464,
        0.005674584,
        0.005898523,
        0.006504338,
        0.005893063,
        0.005967896,
        0.0061056,
        0.006017598,
        0.007500459,
        0.008041236,
        0.0099079,
        0.012297435,
        0.01592606,
        0.021574549,
        0.032780393,
        0.04502082,
        0.054970312,
        0.05930568,
        0.060189471,
        0.057269758,
        0.05486585,
        0.047401041,
        0.038520417,
        0.029925316,
    ],
    [
        0.023567522,
        0.016304584,
        0.012443113,
        0.009961033,
        0.008395854,
        0.007242191,
        0.006314956,
        0.005722235,
        0.005385313,
        0.005197814,
        0.005444756,
        0.0064894,
        0.008409762,
        0.015347201,
        0.025458901,
        0.028619409,
        0.023359044,
        0.014869014,
        0.011900433,
        0.010931316,
        0.010085903,
        0.009253621,
        0.008044246,
        0.007866149,
        0.007665985,
        0.007218414,
        0.00797338,
        0.008005782,
        0.007407311,
        0.008118996,
        0.008648934,
        0.010378068,
        0.013347814,
        0.018541666,
        0.026917161,
        0.035860046,
        0.049702909,
        0.063560224,
        0.069741764,
        0.070609245,
        0.069689625,
        0.069439031,
        0.068785313,
        0.065634051,
        0.062207874,
        0.053986076,
        0.043508937,
        0.033498873,
    ],
    [
        0.025283869,
        0.018061868,
        0.013832406,
        0.01099122,
        0.009057752,
        0.007415348,
        0.006415533,
        0.006118688,
        0.005617255,
        0.005084989,
        0.005552217,
        0.006364787,
        0.00792208,
        0.014440148,
        0.02451,
        0.02993728,
        0.024790064,
        0.016859553,
        0.013140437,
        0.012181571,
        0.010857371,
        0.010621789,
        0.010389982,
        0.010087677,
        0.00981219,
        0.0097001,
        0.01014589,
        0.01052881,
        0.01044948,
        0.011167223,
        0.013610154,
        0.02047533,
        0.035335895,
        0.05409712,
        0.067805633,
        0.074003571,
        0.077948793,
        0.078981046,
        0.077543712,
        0.074620225,
        0.072631194,
        0.070886175,
        0.06972224,
        0.068354439,
        0.063806373,
        0.055709895,
        0.045866391,
        0.035248054,
    ],
    [
        0.030992394,
        0.022532047,
        0.016965296,
        0.013268634,
        0.010662773,
        0.008986943,
        0.007580978,
        0.006707669,
        0.00646337,
        0.006180296,
        0.006229094,
        0.006626391,
        0.00780049,
        0.013149437,
        0.022621172,
        0.033064744,
        0.035953213,
        0.029010413,
        0.023490829,
        0.020477646,
        0.018671663,
        0.017186751,
        0.016526661,
        0.015415424,
        0.014552683,
        0.014347935,
        0.014115058,
        0.013739051,
        0.014944386,
        0.017543021,
        0.021605977,
        0.032100988,
        0.049851633,
        0.063453382,
        0.072579104,
        0.076921792,
        0.079601317,
        0.079548711,
        0.078653413,
        0.076225647,
        0.073936893,
        0.073585752,
        0.071911165,
        0.069220452,
        0.065925982,
        0.059952377,
        0.0510938,
        0.041481111,
    ],
];

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use rstest::*;

    #[fixture]
    fn whole_dwelling_zone() -> JsonValue {
        json!({
            "livingroom_area": 25.0,
            "restofdwelling_area": 100.0,
            "volume": 250.0,
            "Lighting": {"bulbs": [{"count": 10, "power": 3, "efficacy": 150}]},
            "BuildingElement": {
                "roof": {
                    "type": "BuildingElementOpaque",
                    "is_unheated_pitched_roof": true,
                    "colour": "Intermediate",
                    "thermal_resistance_construction": 0.7,
                    "areal_heat_capacity": "Very light",
                    "mass_distribution_class": "IE: Mass divided over internal and external side",
                    "pitch": 45,
                    "orientation360": 90,
                    "base_height": 2.5,
                    "height": 2.5,
                    "width": 10,
                    "area": 20.0,
                }
            },
            "ThermalBridging": {},
        })
    }

    #[fixture]
    fn temp_setback() -> f64 {
        18.0
    }

    #[fixture]
    fn input() -> InputForProcessing {
        let input = json!({
            "HotWaterDemand": {
                "Shower": {
                    "mixer": {
                        "type": "MixerShower",
                        "flowrate": 0,
                        "ColdWaterSource": "mains water",
                    },
                    "IES": {
                        "type": "InstantElecShower",
                        "rated_power": 9.0,
                        "ColdWaterSource": "mains water",
                        "EnergySupply": "mains elec",
                    },
                }
            },
            "Zone": {
                "whole dwelling": {
                    "livingroom_area": 25.0,
                    "restofdwelling_area": 100.0,
                    "volume": 250.0,
                    "Lighting": {"bulbs": [{"count": 10, "power": 3, "efficacy": 150}]},
                    "BuildingElement": {
                        "roof": {
                            "type": "BuildingElementOpaque",
                            "is_unheated_pitched_roof": true,
                            "colour": "Intermediate",
                            "thermal_resistance_construction": 0.7,
                            "areal_heat_capacity": "Very light",
                            "mass_distribution_class": "IE: Mass divided over internal and external side",  // noqa: E501
                            "pitch": 45,
                            "orientation360": 90,
                            "base_height": 2.5,
                            "height": 2.5,
                            "width": 10,
                            "area": 20.0,
                        }
                    },
                    "ThermalBridging": {},
                }
            }
        });

        InputForProcessing { input }
    }

    #[rstest]
    fn test_check_invalid_shower_flowrate(mut input: InputForProcessing) {
        input.input["HotWaterDemand"]["Shower"]["mixer"]["flowrate"] = json!(7.);
        let result = check_shower_flowrate(&input);
        assert!(result.is_err());
        let errror = result.unwrap_err().to_string();
        assert_eq!(
            errror,
            "Invalid flow rate: 7 litres per minute in shower with name 'mixer'"
        )
    }

    #[rstest]
    fn test_check_valid_shower_flowrate(mut input: InputForProcessing) {
        input.input["HotWaterDemand"]["Shower"]["mixer"]["flowrate"] = json!(10.);
        let valid_flowrate = check_shower_flowrate(&input);
        assert!(valid_flowrate.is_ok());
    }

    #[rstest]
    fn test_check_minimum_shower_flowrate(mut input: InputForProcessing) {
        input.input["HotWaterDemand"]["Shower"]["mixer"]["flowrate"] = json!(8.);
        let valid_flowrate = check_shower_flowrate(&input);
        assert!(valid_flowrate.is_ok());
    }
    #[rstest]
    fn test_calc_1_occupant() {
        // test with on occupant and a range of floor areas
        assert_relative_eq!(
            1.075,
            calc_n_occupants(10., 1).unwrap(),
            max_relative = 1e-2
        );
        assert_relative_eq!(
            1.232,
            calc_n_occupants(20., 1).unwrap(),
            max_relative = 1e-2
        );
        assert_relative_eq!(
            1.433,
            calc_n_occupants(50., 1).unwrap(),
            max_relative = 1e-2
        );
        assert_relative_eq!(
            1.437,
            calc_n_occupants(100., 1).unwrap(),
            max_relative = 1e-2
        );
    }

    #[rstest]
    fn test_calc_n_occupants() {
        assert_eq!(2.2472, calc_n_occupants(100., 2).unwrap(),);
        assert_eq!(2.9796, calc_n_occupants(100., 3).unwrap(),);
        assert_eq!(3.3715, calc_n_occupants(100., 4).unwrap(),);
        assert_eq!(3.8997, calc_n_occupants(100., 5).unwrap(),);
        assert_eq!(3.8997, calc_n_occupants(100., 6).unwrap(),);
    }

    #[rstest]
    fn test_calc_n_occupants_invalid_bedrooms() {
        assert!(calc_n_occupants(100., 0).is_err());
    }

    #[rstest]
    fn test_calc_n_occupants_invalid_floor_area() {
        assert!(calc_n_occupants(0., 1).is_err());
        assert!(calc_n_occupants(-1., 1).is_err());
    }

    #[rstest]
    fn test_calc_tfa(mut input: InputForProcessing) {
        // Given a project with a zone area inferred property, and lacking
        // the livingroom_area and restofdwelling_area properties
        input.input["Zone"]["whole dwelling"]["area"] = json!(125.);
        input.input["Zone"]["whole dwelling"]
            .as_object_mut()
            .unwrap()
            .remove_entry("livingroom_area");
        input.input["Zone"]["whole dwelling"]
            .as_object_mut()
            .unwrap()
            .remove_entry("restofdwelling_area");
        // When calc_TFA() is called
        let total_floor_area = calc_tfa(&input).unwrap();
        // Then it returns the total floor area
        let expected_total_floor_area = 125.;
        assert_relative_eq!(total_floor_area, expected_total_floor_area);
    }

    #[rstest]
    fn test_calc_zone_setpoint_fhs(input: InputForProcessing) {
        // Given a zone with a livingroom_area of 40 and a restofdwelling_area of 100
        // When calc_zone_setpoint_fhs() is called
        let setpoint_fhs = calc_zone_setpoint_fhs(&input.input["Zone"]["whole dwelling"]).unwrap();
        // Then it returns the area weighted mean of 21 degC and 20 degC
        // i.e. (21 * 25 + 20 * 100) / (25 + 100) = 20.2
        let expected_setpoint_fhs = 20.2;
        assert_relative_eq!(setpoint_fhs, expected_setpoint_fhs);
    }

    #[rstest]
    fn test_calc_zone_setpoint_fhs_zero_area(mut input: InputForProcessing) {
        // Given a zone with a total area of zero
        input.input["Zone"]["whole dwelling"]["livingroom_area"] = json!(0.);
        input.input["Zone"]["whole dwelling"]["restofdwelling_area"] = json!(0.);

        // When calc_zone_setpoint_fhs() is called
        // Then an exception is raised
        assert!(calc_zone_setpoint_fhs(&input.input["Zone"]["whole dwelling"]).is_err());
    }

    #[rstest]
    fn test_create_zone_area_adds_area_property(mut input: InputForProcessing) {
        // Given a project with a single whole dwelling zone with a livingroom_area
        // and restofdwelling_area totalling 125
        // When create_zone_area() is called        input.input["Zone"]["whole dwelling"]["livingroom_area"] = json!(0.);
        create_zone_area(&mut input).unwrap();
        // Then an area property is added with the expected value
        assert!(input.input["Zone"]["whole dwelling"].get("area").is_some())
    }

    mod test_create_hot_water_distribution {
        use super::*;
        use crate::future_homes_standard::input::InputForProcessing;
        use rstest::{fixture, rstest};
        use serde_json::json;

        #[fixture]
        fn input() -> InputForProcessing {
            let input = json!({
                // Apartment Type 20
            "NumberOfHotTappedRooms": 4,
            "General": {"storeys_in_dwelling": 3},
            "BuildingLength": 11.23,
            "BuildingWidth": 4.55,
            "Zone": {
                "whole dwelling": {
                    "BuildingElement": {
                        // Three valid walls
                        "wall_1": {
                            "type": "BuildingElementOpaque",
                            "base_height": 0,
                            "height": 2.7,
                        },
                        "wall_2": {
                            "type": "BuildingElementOpaque",
                            "base_height": 2.7,
                            "height": 2.7,
                        },
                        "wall_3": {
                            "type": "BuildingElementOpaque",
                            "base_height": 5.4,
                            "height": 2.7,
                        },
                        // One unheated roof (should be ignored)
                        "roof": {
                            "type": "BuildingElementOpaque",
                            "is_unheated_pitched_roof": true,
                            "base_height": 8.1,
                            "height": 3,
                        },
                    }
                }
            },
            "HotWaterDemand": {},
            });
            InputForProcessing { input }
        }

        #[rstest]
        fn test_with_example_dwelling(mut input: InputForProcessing) {
            // Given the test dwelling defined above
            // When distribution is created
            create_hot_water_distribution(&mut input).unwrap();
            // Then the result should contain 2 pipe entries
            let distribution = &input.input["HotWaterDemand"]["Distribution"]
                .as_array()
                .unwrap();
            assert_eq!(distribution.len(), 2);
            assert_eq!(distribution[0]["internal_diameter_mm"], 13.);
            assert_eq!(distribution[1]["internal_diameter_mm"], 20.);
            assert_eq!(distribution[0]["length"], 28.38);
            assert_eq!(distribution[1]["length"], 21.64);
            assert_eq!(distribution[0]["location"], "internal");
            assert_eq!(distribution[1]["location"], "internal");
        }

        #[rstest]
        fn test_with_desnz_h_det_01_de_c_mev(mut input: InputForProcessing) {
            // Given a two storey facsimile input corresponding to a JSON sent by QA
            // "DESN-H-Det-01-DE-cMEV.json"
            input.input["NumberOfHotTappedRooms"] = 2.into();
            input.input["BuildingLength"] = 7.2.into();
            input.input["BuildingWidth"] = 5.9.into();
            input.input["General"]["storeys_in_dwelling"] = 2.into();
            input.input["Zone"]["whole dwelling"]["BuildingElement"]["wall_1"] = json!({
                "type": "BuildingElementOpaque",
                "base_height": 0,
                "height": 2.5,
            });
            input.input["Zone"]["whole dwelling"]["BuildingElement"]["wall_2"] = json!({
                "type": "BuildingElementOpaque",
                "base_height": 2.5,
                "height": 2.68,
            });
            input.input["Zone"]["whole dwelling"]["BuildingElement"]
                .as_object_mut()
                .unwrap()
                .remove("wall_3");
            // When distribution is created
            create_hot_water_distribution(&mut input).unwrap();
            let distribution = input.input["HotWaterDemand"]["Distribution"]
                .as_array()
                .unwrap();

            for pipe in distribution {
                if pipe["internal_diameter_mm"] == 13 {
                    // branching 0.0625 * LL * LW * Nlev  / f (2) = 2.655
                    // shaft 0.038 * LL * LW * building height * (Nwr - 1) / f (2) = 4.1808816
                    assert_eq!(pipe["length"], 6.84);
                } else if pipe["internal_diameter_mm"] == 20 {
                    // main distribution LL + 0.0625 * LL * LW * (Nwr - 1) / f (2) = 8.5275
                    assert_eq!(pipe["length"], 4.93);
                } else {
                    unreachable!();
                }
            }
        }

        #[rstest]
        fn test_non_zero_base_height(mut input: InputForProcessing) {
            // Given all walls have non-zero base_height
            input.input["Zone"]["whole dwelling"]["BuildingElement"] = json!({
                "wall_1": {"type": "BuildingElementOpaque", "base_height": 2.8, "height": 2.5},
                "wall_2": {"type": "BuildingElementOpaque", "base_height": 2.8, "height": 2.4},
                "wall_3": {"type": "BuildingElementOpaque", "base_height": 6, "height": 2.6},
            });

            // When distribution is created
            create_hot_water_distribution(&mut input).unwrap();
            // Then the calculated pipe lengths are as expected
            let expected_distribution = json!([
                {"internal_diameter_mm": 13, "length": 21.68, "location": "internal"},
                {"internal_diameter_mm": 20, "length": 21.64, "location": "internal"},
            ]);
            let actual_distribution = &input.input["HotWaterDemand"]["Distribution"];
            assert_eq!(actual_distribution, &expected_distribution);
        }

        #[rstest]
        fn test_valid_roof_is_included(mut input: InputForProcessing) {
            // Given two valid walls and one valid roof (not unheated)
            input.input["Zone"]["whole dwelling"]["BuildingElement"] = json!({
                "wall_1": {"type": "BuildingElementOpaque", "base_height": 0, "height": 2.8},
                "wall_2": {"type": "BuildingElementOpaque", "base_height": 0, "height": 2.8},
                "roof_1": {
                    "type": "BuildingElementOpaque",
                    "base_height": 3.0,
                    "height": 3.0,
                    "is_unheated_pitched_roof": false,
                },
            });
            // When distribution is created
            create_hot_water_distribution(&mut input).unwrap();
            // Then the roof element should be used to calculate pipe lengths
            let expected_distribution = json!([
                {"internal_diameter_mm": 13, "length": 22.27, "location": "internal"},
                {"internal_diameter_mm": 20, "length": 21.64, "location": "internal"},
            ]);
            let actual_distribution = &input.input["HotWaterDemand"]["Distribution"];
            assert_eq!(actual_distribution, &expected_distribution);
        }

        #[rstest]
        fn test_different_main_dwelling_properties(mut input: InputForProcessing) {
            // Given modified general dwelling information
            input.input["General"]["storeys_in_dwelling"] = 3.into();
            input.input["BuildingLength"] = 12.0.into();
            input.input["BuildingWidth"] = 9.0.into();
            input.input["NumberOfHotTappedRooms"] = 5.into();
            // When distribution is created
            create_hot_water_distribution(&mut input).unwrap();
            // Then all pipe lengths have changed to expected results
            let expected_distribution = json!([
                {"internal_diameter_mm": 13, "length": 76.61, "location": "internal"},
                // 22mm pipes have a different length with new general dwelling information
                {"internal_diameter_mm": 20, "length": 37.5, "location": "internal"},
            ]);
            let actual_distribution = &input.input["HotWaterDemand"]["Distribution"];
            assert_eq!(actual_distribution, &expected_distribution);
        }

        // #[rstest]
        // fn test_zero_wet_rooms(mut input: InputForProcessing) {
        //     // Given a dwelling with zero wet rooms
        //     input.input["NumberOfTappedRooms"] = 0.into();
        //     // When distribution is created
        //     create_hot_water_distribution(&mut input).unwrap();
        //     // Then all pipelengths are zero
        //     let expected_distribution = json!([
        //        {"internal_diameter_mm": 13, "length": 0, "location": "internal"},
        //         {"internal_diameter_mm": 20, "length": 0, "location": "internal"},
        //     ]);
        //     let actual_distribution = &input.input["HotWaterDemand"]["Distribution"];
        //     assert_eq!(actual_distribution, &expected_distribution);
        // }

        #[rstest]
        fn test_one_hot_tapped_room(mut input: InputForProcessing) {
            // Given a dwelling with 1 hot tapped room (the minimum allowed value)
            input.input["General"]["storeys_in_dwelling"] = 3.into();
            input.input["BuildingLength"] = 12.0.into();
            input.input["BuildingWidth"] = 9.0.into();
            input.input["NumberOfHotTappedRooms"] = 1.into();
            // When distribution is created
            create_hot_water_distribution(&mut input).unwrap();
            // Then the long pipelengths is set to 0.1, as 0 pipe lengths aren't allowed by the core
            // the small pipework value is calculated correctly
            // small_vertical_pipe_length = main_shaft_pipe_length * non_kitchen_tapped_rooms = 0
            // branching_pipe_length = (
            //     building_length * building_width * number_of_storeys * branch_circuit_factor
            // )
            // 12 * 9 * 3 * 0.0625 = 20.25
            // small_pipe_length=(branching_pipe_length + small_vertical_pipe_length) / reduction_factor
            // (20.25 + 0) / 2 = 10.12 (2dp)
            let expected_distribution = json!([
                {"internal_diameter_mm": 13, "length": 10.12, "location": "internal"},
                {"internal_diameter_mm": 20, "length": 0.1, "location": "internal"},
            ]);
            let actual_distribution = &input.input["HotWaterDemand"]["Distribution"];
            assert_eq!(actual_distribution, &expected_distribution);
        }
    }

    mod test_create_water_heating_pattern {
        use super::*;
        use crate::future_homes_standard::input::InputForProcessing;
        use rstest::{fixture, rstest};
        use serde_json::json;

        #[fixture]
        fn input() -> InputForProcessing {
            let input = json!({
                "Control": {},
                "EnergySupply": {"mains elec": {"fuel": "electricity", "is_export_capable": true}},
                "HotWaterSource": {
                    "hw cylinder": {
                        "type": "StorageTank",
                        "volume": 80.0,
                        "daily_losses": 1.68,
                        "ColdWaterSource": "header tank",
                        "HeatSource": {
                            "immersion": {
                                "type": "ImmersionHeater",
                                "power": 3.0,
                                "EnergySupply": "mains elec",
                                "heater_position": 0.1,
                                "thermostat_position": 0.33,
                            }
                        },
                    }
                },
            });

            InputForProcessing { input }
        }

        #[rstest]
        fn test_storage_tank_gets_controls(mut input: InputForProcessing) {
            // Given a dwelling with an ordinary StorageTank fed by header tank and immersion
            // When water heating pattern is created
            create_water_heating_pattern(&mut input).unwrap();
            // Then the heating pattern reflects:
            //   * a min of 60C for first 2 hours followed by 52C
            //   * a max of 60C
            assert_eq!(
                input.input["HotWaterSource"]["hw cylinder"]["HeatSource"]["immersion"]
                    ["Controlmax"],
                "_HW_max_temp"
            );
            assert_eq!(
                input.input["HotWaterSource"]["hw cylinder"]["HeatSource"]["immersion"]
                    ["Controlmin"],
                "_HW_min_temp"
            );
            assert_eq!(
                input.input["Control"]["_HW_min_temp"],
                json!({
                    "schedule": {
                        "main": [{"repeat": 53, "value": "week"}],
                        "week": [{"repeat": 6, "value": "other_day"}, {"repeat": 1, "value": "sunday"}],
                        "other_day": [{"repeat": 48, "value": 52.0}],
                        "sunday": [{"repeat": 4, "value": 60.0}, {"repeat": 44, "value": 52.0}],
                    },
                    "start_day": 0,
                    "time_series_step": 0.5,
                    "type": "SetpointTimeControl",
                })
            );
            assert_eq!(
                input.input["Control"]["_HW_max_temp"],
                json!({
                    "schedule": {
                        "day": [{"repeat": 48, "value": 60.0}],
                        "main": [{"repeat": 365, "value": "day"}],
                    },
                    "start_day": 0,
                    "time_series_step": 0.5,
                    "type": "SetpointTimeControl",
                })
            );
        }

        #[rstest]
        fn test_solar_thermal_has_no_min(mut input: InputForProcessing) {
            // Given a dwelling with an ordinary StorageTank fed by header tank heated by solar thermal
            input.input["HotWaterSource"]["hw cylinder"]["HeatSource"] = json!({
                "SolarThermalSystem": {
                    "type": "SolarThermalSystem",
                    "sol_loc": "OUT",
                    "area_module": 3,
                    "modules": 1,
                    "peak_collector_efficiency": 0.8,
                    "incidence_angle_modifier": 0.9,
                    "first_order_hlc": 3.5,
                    "second_order_hlc": 0,
                    "collector_mass_flow_rate": 1,
                    "power_pump": 0.1,
                    "power_pump_control": 0.01,
                    "EnergySupply": "mains elec",
                    "tilt": 30,
                    "orientation360": 180,
                    "solar_loop_piping_hlc": 0.5,
                    "heater_position": 0.08,
                    "thermostat_position": 0.33,
                }
            });
            // When water heating pattern is created
            create_water_heating_pattern(&mut input).unwrap();
            // Then only Controlmax is set
            assert!(input.input["HotWaterSource"]["hw cylinder"]["HeatSource"]
                ["SolarThermalSystem"]
                .get("Controlmin")
                .is_none());
            assert_eq!(
                input.input["HotWaterSource"]["hw cylinder"]["HeatSource"]["SolarThermalSystem"]
                    ["Controlmax"],
                "_HW_max_temp"
            );
        }

        #[rstest]
        fn test_smart_tank_gets_controls(mut input: InputForProcessing) {
            // Given a dwelling with a SmartHotWaterTank fed by a header tank and immersion
            input.input["HotWaterSource"]["hw cylinder"]["type"] = json!("SmartHotWaterTank");

            // When water heating pattern is created
            create_water_heating_pattern(&mut input).unwrap();

            // Then the heating pattern reflects a max temperature of 60C for the tank always
            // and controls with a min/max state of charge:
            assert_eq!(
                input.input["HotWaterSource"]["hw cylinder"]["HeatSource"]["immersion"]
                    ["Controlmax"],
                "_HW_smart_hot_water_tank_max_soc"
            );
            assert_eq!(
                input.input["HotWaterSource"]["hw cylinder"]["HeatSource"]["immersion"]
                    ["Controlmin"],
                "_HW_smart_hot_water_tank_min_soc"
            );
            assert_eq!(
                input.input["HotWaterSource"]["hw cylinder"]["temp_setpnt_max"],
                "_HW_smart_hot_water_tank_temp_max"
            );
            assert_eq!(
                input.input["Control"]["_HW_smart_hot_water_tank_max_soc"],
                json!({
                    "schedule": {
                        "day": [
                            {"repeat": 2, "value": 1.0},
                            {"repeat": 1, "value": 0.6},
                            {"repeat": 4, "value": 0.5},
                            {"repeat": 17, "value": 0.6},
                        ],
                        "main": [{"repeat": 365, "value": "day"}],
                    },
                    "start_day": 0,
                    "time_series_step": 1,
                    "type": "SetpointTimeControl",
                })
            );
            assert_eq!(
                input.input["Control"]["_HW_smart_hot_water_tank_min_soc"],
                json!({
                    "schedule": {
                        "day": [
                            {"repeat": 2, "value": 1.0},
                            {"repeat": 1, "value": 0.1},
                            {"repeat": 4, "value": 0.5},
                            {"repeat": 17, "value": 0.1},
                        ],
                        "main": [{"repeat": 365, "value": "day"}],
                    },
                    "start_day": 0,
                    "time_series_step": 1,
                    "type": "SetpointTimeControl",
                })
            );
            assert_eq!(
                input.input["Control"]["_HW_smart_hot_water_tank_temp_max"],
                json!({
                    "schedule": {"main": [{"repeat": 8760, "value": 60.0}]},
                    "start_day": 0,
                    "time_series_step": 1,
                    "type": "SetpointTimeControl"
                })
            );
        }

        #[rstest]
        fn test_preheated_tank_gets_controls(mut input: InputForProcessing) {
            // Given a dwelling with a preheated tank (fed by a header tank)
            input.input["PreHeatedWaterSource"] = json!({
                "preheated tank": {
                    "volume": 80.0,
                    "daily_losses": 1.68,
                    "ColdWaterSource": "header tank",
                    "HeatSource": {
                        "immersion": {
                            "type": "ImmersionHeater",
                            "power": 3.0,
                            "EnergySupply": "mains elec",
                            "heater_position": 0.1,
                            "thermostat_position": 0.33,
                        }
                    },
                }
            });

            // When water heating pattern is created
            create_water_heating_pattern(&mut input).unwrap();

            // Then it receives identical controls to a StorageTank
            assert_eq!(
                input.input["PreHeatedWaterSource"]["preheated tank"]["HeatSource"]["immersion"]
                    ["Controlmax"],
                "_HW_max_temp"
            );
            assert_eq!(
                input.input["PreHeatedWaterSource"]["preheated tank"]["HeatSource"]["immersion"]
                    ["Controlmin"],
                "_HW_min_temp"
            );
        }
    }

    mod test_create_hot_water_demand {
        use super::*;
        use crate::future_homes_standard::input::InputForProcessing;
        use serde_json::json;

        #[test]
        fn test_initialises_missing_hot_water_demand() {
            // Given a project_dict with no HotWaterDemand
            let mut input = InputForProcessing { input: json!({}) };

            // When create_hot_water_demand is called
            create_hot_water_demand(&mut input).unwrap();

            // Then HotWaterDemand and sub-keys should be initialised
            assert_eq!(input.input["HotWaterDemand"]["Shower"], json!({}));
            assert_eq!(input.input["HotWaterDemand"]["Bath"], json!({}));
            assert_eq!(input.input["HotWaterDemand"]["Other"], json!({}));
        }

        #[test]
        fn test_initialises_missing_sub_keys() {
            // Given a project_dict with no HotWaterDemand sub-keys
            let mut input = InputForProcessing {
                input: json!({"HotWaterDemand": {}}),
            };

            // When create_hot_water_demand is called
            create_hot_water_demand(&mut input).unwrap();

            // Then HotWaterDemand and sub-keys should be initialised
            assert_eq!(input.input["HotWaterDemand"]["Shower"], json!({}));
            assert_eq!(input.input["HotWaterDemand"]["Bath"], json!({}));
            assert_eq!(input.input["HotWaterDemand"]["Other"], json!({}));
        }

        #[test]
        fn test_preserves_existing_keys() {
            // Given existing values under HotWaterDemand
            let mut original_input = InputForProcessing {
                input: json!({
                    "HotWaterDemand": {
                        "Shower": {
                            "mixer": {
                                "type": "MixerShower",
                                "flowrate": 8.0,
                                "ColdWaterSource": "mains water",
                            }
                        },
                        "Bath": {
                            "medium": {"size": 100, "ColdWaterSource": "header tank", "flowrate": 8.0}
                        },
                        "Other": {"other": {"flowrate": 8.0, "ColdWaterSource": "header tank"}},
                    }
                }),
            };

            let input = original_input.clone();

            // When create_hot_water_demand is called
            create_hot_water_demand(&mut original_input).unwrap();

            // Then the existing values should remain unchanged
            assert_eq!(input, original_input);
        }
    }

    mod test_create_custom_energy_supply_factors {
        use super::*;
        use crate::future_homes_standard::input::InputForProcessing;
        use serde_json::json;

        #[test]
        fn test_sets_custom_energy_supplies() {
            // Given a custom energy supply specified for a heat network
            let mut input = InputForProcessing {
                input: json!({
                    "EnergySupply": {},
                    "HeatSourceWet": {
                        "heat network": {
                            "type": "HIU",
                            "is_heat_network": true,
                            "heat_network_type": "sleeved DHN",
                            "HIU_daily_loss": 1,
                            "power_max": 1,
                            "building_level_distribution_losses": 1,
                            "EnergySupply": {
                                "name": "custom_heat_network_supply",
                                "factor": {
                                    "Emissions Factor kgCO2e/kWh": 1,
                                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 1,
                                    "Primary Energy Factor kWh/kWh delivered": 1,
                                },
                                "is_export_capable": false,
                            },
                        }
                    },
                }),
            };

            // When create_custom_energy_supply is called for non notional mode
            let stored_factors = create_custom_energy_supply_factors(&mut input).unwrap();

            // Then the input is mutated such that a custom fuel energy supply is created
            assert_eq!(
                input.input["EnergySupply"]["custom_heat_network_supply"],
                json!({"fuel": "custom", "is_export_capable": false})
            );
            // And referenced by the heat network
            assert_eq!(
                input.input["HeatSourceWet"]["heat network"]["EnergySupply"],
                "custom_heat_network_supply"
            );
            // And the factors are stored for later postprocessing
            assert_eq!(
                stored_factors["custom_heat_network_supply"],
                serde_json::from_value(json!({
                    "Emissions Factor kgCO2e/kWh": 1,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 1,
                    "Primary Energy Factor kWh/kWh delivered": 1,
                }))
                .unwrap()
            );
        }

        #[test]
        fn test_handles_custom_energy_supply_heat_network_for_heat_pump() {
            // Given a HeatSourceWet of type HeatPump with a
            // source_type of HeatNetwork and a custom EnergySupply_heat_network
            let mut input = InputForProcessing {
                input: json!({
                    "EnergySupply": {"mains elec": {"fuel": "electricity", "is_export_capable": true}},
                    "HeatSourceWet": {
                        "heat pump": {
                            "type": "HeatPump",
                            "EnergySupply": "mains elec",
                            "EnergySupply_heat_network": {
                                "name": "custom_heat_network_supply",
                                "factor": {
                                    "Emissions Factor kgCO2e/kWh": 0.99,
                                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 0.98,
                                    "Primary Energy Factor kWh/kWh delivered": 0.97,
                                },
                                "is_export_capable": false,
                            },
                            "is_heat_network": false,
                            "source_type": "HeatNetwork",
                            "temp_distribution_heat_network": 20.0,
                            "sink_type": "Water",
                            "backup_ctrl_type": "TopUp",
                            "modulating_control": true,
                            "min_modulation_rate_35": 0.35,
                            "min_modulation_rate_55": 0.4,
                            "time_constant_onoff_operation": 140,
                            "temp_return_feed_max": 70.0,
                            "temp_lower_operating_limit": -5.0,
                            "min_temp_diff_flow_return_for_hp_to_operate": 0.0,
                            "var_flow_temp_ctrl_during_test": true,
                            "power_heating_circ_pump": 0.015,
                            "power_source_circ_pump": 0.01,
                            "power_standby": 0.015,
                            "power_crankcase_heater": 0.01,
                            "power_off": 0.015,
                            "power_max_backup": 3.0,
                            "test_data_EN14825": [
                                {
                                    "test_letter": "A",
                                    "capacity": 8.4,
                                    "cop": 4.6,
                                    "design_flow_temp": 35,
                                    "temp_outlet": 34,
                                    "temp_source": 20,
                                    "temp_test": -7,
                                }
                            ],
                        }
                    },
                }),
            };

            // When create_custom_energy_supply is called for non notional mode
            let stored_factors = create_custom_energy_supply_factors(&mut input).unwrap();

            // Then the dictionary is mutated such that a custom fuel energy supply is created
            assert_eq!(
                input.input["EnergySupply"]["custom_heat_network_supply"],
                json!({"fuel": "custom", "is_export_capable": false})
            );
            // And referenced by the heat pump
            assert_eq!(
                input.input["HeatSourceWet"]["heat pump"]["EnergySupply_heat_network"],
                "custom_heat_network_supply"
            );
            // And the factors are stored for later postprocessing
            assert_eq!(
                stored_factors["custom_heat_network_supply"],
                serde_json::from_value(json!({
                    "Emissions Factor kgCO2e/kWh": 0.99,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 0.98,
                    "Primary Energy Factor kWh/kWh delivered": 0.97,
                }))
                .unwrap()
            );
        }

        #[test]
        fn test_handles_both_custom_energy_supplies_for_heat_pump_that_is_itself_a_heat_network() {
            // Given a HeatSourceWet of type HeatPump that is itself a heat network, with a
            // source_type that is also HeatNetwork and a custom EnergySupply_heat_network
            // and a custom EnergySupply
            let mut input = InputForProcessing {
                input: json!({
                    "EnergySupply": {"mains elec": {"fuel": "electricity", "is_export_capable": true}},
                    "HeatSourceWet": {
                        "heat pump": {
                            "type": "HeatPump",
                            "EnergySupply": {
                                "name": "custom_heat_pump_supply",
                                "factor": {
                                    "Emissions Factor kgCO2e/kWh": 1,
                                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 2,
                                    "Primary Energy Factor kWh/kWh delivered": 0.5,
                                },
                                "is_export_capable": false,
                            },
                            "EnergySupply_heat_network": {
                                "name": "custom_heat_network_supply",
                                "factor": {
                                    "Emissions Factor kgCO2e/kWh": 0.99,
                                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 0.98,
                                    "Primary Energy Factor kWh/kWh delivered": 0.97,
                                },
                                "is_export_capable": false,
                            },
                            "is_heat_network": true,
                            "heat_network_type": "sleeved DHN",
                            "source_type": "HeatNetwork",
                            "temp_distribution_heat_network": 20.0,
                            "sink_type": "Water",
                            "backup_ctrl_type": "TopUp",
                            "modulating_control": true,
                            "min_modulation_rate_35": 0.35,
                            "min_modulation_rate_55": 0.4,
                            "time_constant_onoff_operation": 140,
                            "temp_return_feed_max": 70.0,
                            "temp_lower_operating_limit": -5.0,
                            "min_temp_diff_flow_return_for_hp_to_operate": 0.0,
                            "var_flow_temp_ctrl_during_test": true,
                            "power_heating_circ_pump": 0.015,
                            "power_source_circ_pump": 0.01,
                            "power_standby": 0.015,
                            "power_crankcase_heater": 0.01,
                            "power_off": 0.015,
                            "power_max_backup": 3.0,
                            "test_data_EN14825": [
                                {
                                    "test_letter": "A",
                                    "capacity": 8.4,
                                    "cop": 4.6,
                                    "design_flow_temp": 35,
                                    "temp_outlet": 34,
                                    "temp_source": 20,
                                    "temp_test": -7,
                                }
                            ],
                        }
                    },
                }),
            };

            // When create_custom_energy_supply is called for non notional mode
            let stored_factors = create_custom_energy_supply_factors(&mut input).unwrap();

            // Then the dictionary is mutated such that a custom fuel energy supply is created
            assert_eq!(
                input.input["EnergySupply"]["custom_heat_network_supply"],
                json!({"fuel": "custom", "is_export_capable": false})
            );

            // And referenced by the heat pump
            assert_eq!(
                input.input["HeatSourceWet"]["heat pump"]["EnergySupply"],
                "custom_heat_pump_supply"
            );
            assert_eq!(
                input.input["HeatSourceWet"]["heat pump"]["EnergySupply_heat_network"],
                "custom_heat_network_supply"
            );

            // And the factors are stored for later postprocessing
            assert_eq!(
                stored_factors["custom_heat_pump_supply"],
                serde_json::from_value(json!({
                    "Emissions Factor kgCO2e/kWh": 1,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 2,
                    "Primary Energy Factor kWh/kWh delivered": 0.5,
                }))
                .unwrap()
            );
            assert_eq!(
                stored_factors["custom_heat_network_supply"],
                serde_json::from_value(json!({
                    "Emissions Factor kgCO2e/kWh": 0.99,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 0.98,
                    "Primary Energy Factor kWh/kWh delivered": 0.97,
                }))
                .unwrap()
            );
        }

        #[test]
        fn test_raises_error_if_custom_energy_supply_name_already_exists() {
            // Given a custom energy supply specified for a heat network with
            // a name that is the same as an existing energy supply
            let mut input = InputForProcessing {
                input: json!({
                    "EnergySupply": {"mains elec": {"fuel": "electricity", "is_export_capable": true}},
                    "HeatSourceWet": {
                        "heat network": {
                            "type": "HIU",
                            "is_heat_network": true,
                            "heat_network_type": "sleeved DHN",
                            "HIU_daily_loss": 1,
                            "power_max": 1,
                            "building_level_distribution_losses": 1,
                            "EnergySupply": {
                                "name": "mains elec",  // conflicts with existing energy supply
                                "factor": {
                                    "Emissions Factor kgCO2e/kWh": 1,
                                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 1,
                                    "Primary Energy Factor kWh/kWh delivered": 1,
                                },
                                "is_export_capable": false,
                            },
                        }
                    },
                }),
            };

            let result = create_custom_energy_supply_factors(&mut input);
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().to_string(),
                "An EnergySupply named 'mains elec' already exists. Unable to add a custom EnergySupply for HeatSourceWet 'heat network' with the same name."
            );
        }

        #[test]
        fn test_raises_error_if_custom_energy_supply_heat_network_name_already_exists() {
            // Given a HeatSourceWet of type HeatPump with a
            // source_type of HeatNetwork and a custom EnergySupply_heat_network
            // with a name that is the same as an existing energy supply
            let mut input = InputForProcessing {
                input: json!({
                    "EnergySupply": {"mains elec": {"fuel": "electricity", "is_export_capable": true}},
                    "HeatSourceWet": {
                        "heat pump": {
                            "type": "HeatPump",
                            "EnergySupply": "mains elec",
                            "EnergySupply_heat_network": {
                                "name": "mains elec",
                                "factor": {
                                    "Emissions Factor kgCO2e/kWh": 0.99,
                                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 0.98,
                                    "Primary Energy Factor kWh/kWh delivered": 0.97,
                                },
                                "is_export_capable": false,
                            },
                            "is_heat_network": false,
                            "source_type": "HeatNetwork",
                            "temp_distribution_heat_network": 20.0,
                            "sink_type": "Water",
                            "backup_ctrl_type": "TopUp",
                            "modulating_control": true,
                            "min_modulation_rate_35": 0.35,
                            "min_modulation_rate_55": 0.4,
                            "time_constant_onoff_operation": 140,
                            "temp_return_feed_max": 70.0,
                            "temp_lower_operating_limit": -5.0,
                            "min_temp_diff_flow_return_for_hp_to_operate": 0.0,
                            "var_flow_temp_ctrl_during_test": true,
                            "power_heating_circ_pump": 0.015,
                            "power_source_circ_pump": 0.01,
                            "power_standby": 0.015,
                            "power_crankcase_heater": 0.01,
                            "power_off": 0.015,
                            "power_max_backup": 3.0,
                            "test_data_EN14825": [
                                {
                                    "test_letter": "A",
                                    "capacity": 8.4,
                                    "cop": 4.6,
                                    "design_flow_temp": 35,
                                    "temp_outlet": 34,
                                    "temp_source": 20,
                                    "temp_test": -7,
                                }
                            ],
                        }
                    },
                }),
            };

            let result = create_custom_energy_supply_factors(&mut input);
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().to_string(),
                "An EnergySupply named 'mains elec' already exists. Unable to add a custom EnergySupply_heat_network for HeatSourceWet 'heat pump' with the same name."
            );
        }
    }

    mod apply_defaults {
        use super::*;

        #[test]
        fn test_adds_floor_pitch() {
            // Given a floor building element
            let mut input = InputForProcessing {
                input: json!({
                    "InfiltrationVentilation": {"Vents": {}},
                    "Zone": {"a": {"BuildingElement": {"floor": {"type": "BuildingElementGround"}}}},
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // when apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then a pitch value of 180 is added to the floor
            assert_eq!(
                input.input["Zone"],
                json!({"a": {"BuildingElement": {"floor": {"type": "BuildingElementGround", "pitch": 180}}}})
            );
        }

        #[test]
        fn test_adds_orientation_to_flat_opaques() {
            // Given a floor building element
            let mut input = InputForProcessing {
                input: json!({
                    "InfiltrationVentilation": {"Vents": {}},
                    "Zone": {
                        "a": {"BuildingElement": {"floor": {"type": "BuildingElementOpaque", "pitch": 0}}}
                    },
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then a orientation360 value of 180 is added to the flat element and intermediate colour
            assert_eq!(
                input.input["Zone"],
                json!({
                    "a": {
                        "BuildingElement": {
                            "floor": {
                                "type": "BuildingElementOpaque",
                                "pitch": 0,
                                "orientation360": 180,
                                "colour": "Intermediate",
                            }
                        }
                    }
                })
            );
        }

        #[test]
        fn test_adds_orientation_to_flat_transparent_elements() {
            // Given a window building element with a flat pitch, indicating a skylight
            let mut input = InputForProcessing {
                input: json!({
                    "InfiltrationVentilation": {"Vents": {}},
                    "Zone": {
                        "a": {
                            "BuildingElement": {
                                "window": {"type": "BuildingElementTransparent", "pitch": 0}
                            }
                        }
                    },
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then an orientation360 value of 180 is added to the element
            assert_eq!(
                input.input["Zone"],
                json!({
                    "a": {
                        "BuildingElement": {
                            "window": {
                                "type": "BuildingElementTransparent",
                                "pitch": 0,
                                "orientation360": 180,
                            }
                        }
                    }
                })
            );
        }

        #[test]
        fn test_adds_vent_opening_ratio_init() {
            // Given a project dict with InfiltrationVentilation
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {}},
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then vent_opening_ratio_init is added to the InfiltrationVentilation object and set to 1
            assert_eq!(
                input.input["InfiltrationVentilation"]["vent_opening_ratio_init"],
                json!(1),
            );
        }

        #[test]
        fn test_adds_pressure_difference_ref() {
            // Given a project dict with InfiltrationVentilation and one vent
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {"vent1": {}}},
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then pressure_difference_ref is added to the Vent object and set to 20
            assert_eq!(
                input.input["InfiltrationVentilation"]["Vents"]["vent1"],
                json!({"pressure_difference_ref": 20})
            );
        }

        #[test]
        fn test_adds_sup_air_flw_ctrl_and_sup_air_temp_ctrl() {
            // Given a project dict with InfiltrationVentilation and one mech vent
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {}, "MechanicalVentilation": {"mechvent1": {}}},
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then sup_air_flw_ctrl and sup_air_temp_ctrl are added as "ODA" and "NO_CTRL" respectively
            assert_eq!(
                input.input["InfiltrationVentilation"]["MechanicalVentilation"]["mechvent1"],
                json!({"sup_air_flw_ctrl": "ODA", "sup_air_temp_ctrl": "NO_CTRL"})
            );
        }

        #[test]
        fn test_adds_battery_age() {
            // Given a project dict with Energy supply and one ElectricBattery
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {}},
                    "EnergySupply": {"supply1": {"fuel": "electricity", "ElectricBattery": {}}},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then battery_age is added to the ElectricBattery object and set to 0
            assert_eq!(
                input.input["EnergySupply"]["supply1"]["ElectricBattery"]["battery_age"],
                json!(0)
            );
        }

        #[test]
        fn test_adds_battery_grid_charging() {
            // Given a project dict with Energy supply and one ElectricBattery
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {}},
                    "EnergySupply": {"supply1": {"fuel": "electricity", "ElectricBattery": {}}},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then grid_charging_possible is added and set to False
            assert_eq!(
                input.input["EnergySupply"]["supply1"]["ElectricBattery"]["grid_charging_possible"],
                json!(false)
            );
        }

        #[test]
        fn test_adds_bath_flowrate() {
            // Given a project dict with HotWaterDemand and one Bath
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {}},
                    "EnergySupply": {},
                    "HotWaterDemand": {"Bath": {"bath1": {}}},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then flowrate is added to the Bath object and set to 12
            assert_eq!(
                input.input["HotWaterDemand"]["Bath"]["bath1"],
                json!({"flowrate": 12})
            );
        }

        #[test]
        fn test_adds_is_export_capable_gas() {
            // Given a project dict with Energy supply and gas fuel type
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {}},
                    "EnergySupply": {"supply1": {"fuel": "gas"}},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then is_export_capable is added and set to false
            assert_eq!(
                input.input["EnergySupply"]["supply1"],
                json!({"fuel": "gas", "is_export_capable": false})
            );
        }

        #[test]
        fn test_adds_is_export_capable_electricity() {
            // Given a project dict with Energy supply and electricity fuel type
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {}},
                    "EnergySupply": {"supply1": {"fuel": "electricity"}},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then is_export_capable is added and set to true
            assert_eq!(
                input.input["EnergySupply"]["supply1"],
                json!({"fuel": "electricity", "is_export_capable": true})
            );
        }

        #[test]
        fn test_adds_is_export_capable_electricity_override() {
            // Given a project dict with an energy supply with electricity fuel type
            // Where the user has specified the value for is_export_capable
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {},
                    "InfiltrationVentilation": {"Vents": {}},
                    "EnergySupply": {"supply1": {"fuel": "electricity", "is_export_capable": false}},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then the user specified value is not overridden
            assert_eq!(
                input.input["EnergySupply"]["supply1"],
                json!({"fuel": "electricity", "is_export_capable": false})
            );
        }

        #[test]
        fn test_adds_state_of_charge_init() {
            // Given a electric storage heater
            let mut input = InputForProcessing {
                input: json!({
                    "InfiltrationVentilation": {"Vents": {}},
                    "Zone": {},
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {"a": {"type": "ElecStorageHeater"}},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then a state_of_charge_init of 1.0 is added to the heater
            assert_eq!(
                input.input["SpaceHeatSystem"]["a"]["state_of_charge_init"],
                json!(1.0)
            );
        }

        #[test]
        fn test_adds_temp_init_for_heat_batteries() {
            // Given a heat battery
            let mut input = InputForProcessing {
                input: json!({
                    "InfiltrationVentilation": {"Vents": {}},
                    "Zone": {},
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                    "HeatSourceWet": {
                        "a": {"type": "HeatBattery", "max_temperature": 80, "battery_type": "pcm"}
                    },
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then a temp_init equal to the max_temperature is added to the battery (80)
            assert_eq!(input.input["HeatSourceWet"]["a"]["temp_init"], json!(80));
        }

        #[test]
        fn test_adds_time_delay_backup_for_heat_pumps() {
            // Given a heat pump with a backup_ctrl_type of not None
            let mut input = InputForProcessing {
                input: json!({
                    "InfiltrationVentilation": {"Vents": {}},
                    "Zone": {},
                    "EnergySupply": {},
                    "HotWaterDemand": {},
                    "SpaceHeatSystem": {},
                    "HeatSourceWet": {"a": {"type": "HeatPump", "backup_ctrl_type": "TopUp"}},
                }),
            };
            // When apply_defaults is called
            apply_defaults(&mut input).unwrap();
            // Then a time_delay_backup of 1.0 is added to the heat pump
            assert_eq!(
                input.input["HeatSourceWet"]["a"]["time_delay_backup"],
                json!(1.0)
            );
        }
    }

    mod calc_n_occupants {
        use super::*;

        #[test]
        fn test_invalid_tfa_raises() {
            // Given a total floor area of 0
            // which is possible to get from a valid input
            let tfa = 0.0;
            let n_beds = 1usize;
            // When calc_N_occupants is called
            // Then an error is returned
            let result = calc_n_occupants(tfa, n_beds);
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Invalid total floor area"));
        }
    }

    mod create_lighting_gains {
        use super::*;

        #[test]
        fn test_total_wattage_0_raises() {
            // Given a valid project_dict input with 0 total bulb wattage (power)
            let tfa = 100.0;
            let n_occupants = 5.;
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {
                        "whole dwelling": {
                            "Lighting": {
                                "bulbs": [
                                    {"count": 20, "power": 0, "efficacy": 150},
                                    {"count": 20, "power": 0, "efficacy": 50},
                                ]
                            }
                        }
                    }
                }),
            };
            // When create_lighting_gains is called
            // Then an error is returned
            let result = create_lighting_gains(&mut input, tfa, n_occupants);
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().to_string(),
                "Invalid total wattage in zone whole dwelling, cannot equal 0."
            );
        }

        #[test]
        fn test_lighting_efficacy_0_raises() {
            // Given a valid project_dict input with lighting efficacy 0, total floor area of 100
            // and 5 occupants
            let tfa = 100.0;
            let n_occupants = 5.;
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {
                        "whole dwelling": {
                            "area": 100.0,
                            "Lighting": {
                                "bulbs": [
                                    {"count": 20, "power": 3, "efficacy": 0},
                                    {"count": 2, "power": 30, "efficacy": 0},
                                ]
                            },
                        }
                    }
                }),
            };
            // When create_lighting_gains is called
            // Then an error is returned
            let result = create_lighting_gains(&mut input, tfa, n_occupants);
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().to_string(),
                "Invalid lighting efficacy calculated from bulb details for all zones, cannot equal 0."
            );
        }

        #[test]
        fn test_total_area_0_raises() {
            // Given a zone with area property equal to 0
            // which is possible to get from a valid input
            let tfa = 100.0;
            let n_occupants = 5.;
            let mut input = InputForProcessing {
                input: json!({
                    "Zone": {
                        "whole dwelling": {
                            "area": 0.0,
                            "Lighting": {"bulbs": [{"count": 20, "power": 3, "efficacy": 20}]},
                        }
                    }
                }),
            };
            // When create_lighting_gains is called
            // Then an error is returned
            let result = create_lighting_gains(&mut input, tfa, n_occupants);
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().to_string(),
                "Invalid/missing value calculated for total area across zones, cannot equal 0."
            );
        }
    }

    // test_empty_propensity_raises_zero_division_error skipped as our type constraints do not allow for empty propensities to be provided - error will be caught before this

    mod appliance_kwh_cycle_loading_factor {
        use super::*;

        #[test]
        fn test_demand_not_specified_raises() {
            // Given a project dict that has an appliance without a demand property
            let input = InputForProcessing {
                input: json!({"Appliances": {"Clothes_washing": {}}}),
            };
            let appliance_name = "Clothes_washing";
            let appliance_map = Default::default();

            // When appliance_kWhcycle_loadingfactor is called
            // Then an error is returned
            let result = appliance_kwh_cycle_loading_factor(&input, appliance_name, &appliance_map);
            assert!(result.is_err());
            assert!(
                result.unwrap_err().to_string().contains("demand must be specified as one of 'kWh_per_cycle', 'kWh_per_100cycle' or 'kWh_per_annum'"),
            );
        }

        #[test]
        fn test_clothes_drying_no_spin_class_applies_no_adjustment() {
            // Given a clothes drying appliance with a kWh/cycle specified
            // and a clothes washing appliance present, but without spin class
            let input = InputForProcessing {
                input: json!({
                    "Appliances": {
                        "Clothes_drying": {"kWh_per_cycle": 1.0, "kg_load": 5.0},
                        "Clothes_washing": {
                            // No spin_dry_efficiency_class -> assume 60% moisture, no correction
                        },
                    }
                }),
            };
            let appliance_name = "Clothes_drying";
            let appliance_map = [(
                "Clothes_drying",
                ApplianceUseProfile {
                    util_unit: 0.0,
                    use_data: Some(ApplianceUseData {
                        use_metric: 0,
                        clothes_use_data: Some(ClothesUseData {
                            standard_load_kg: 6.0,
                        }),
                        _standard_use: None,
                        duration: 0.0,
                        duration_deviation: 0.0,
                    }),
                    standby: 0.0,
                    gains_frac: 0.0,
                    prof: vec![],
                },
            )]
            .into();

            // When appliance_kWhcycle_loadingfactor is called
            let (kwh_cycle, loadingfactor) =
                appliance_kwh_cycle_loading_factor(&input, appliance_name, &appliance_map).unwrap();
            // Then kWh/cycle is unchanged (adjustment = 1.0)
            assert_eq!(kwh_cycle, 1.0);
            // And loading factor is standard_load / kg_load
            assert_eq!(loadingfactor, 1.2); // 6.0 / 5.0
        }

        #[test]
        fn test_clothes_drying_spin_class_f_applies_adjustment() {
            // Given a clothes drying appliance with kWh/cycle
            // and a clothes washing appliance with spin class F
            let input = InputForProcessing {
                input: json!({
                    "Appliances": {
                        "Clothes_drying": {"kWh_per_cycle": 1.0, "kg_load": 5.0},
                        "Clothes_washing": {"spin_dry_efficiency_class": "F"},
                    }
                }),
            };
            let appliance_name = "Clothes_drying";
            let appliance_map = [(
                "Clothes_drying",
                ApplianceUseProfile {
                    util_unit: 0.0,
                    use_data: Some(ApplianceUseData {
                        use_metric: 0,
                        clothes_use_data: Some(ClothesUseData {
                            standard_load_kg: 6.0,
                        }),
                        _standard_use: None,
                        duration: 0.0,
                        duration_deviation: 0.0,
                    }),
                    standby: 0.0,
                    gains_frac: 0.0,
                    prof: vec![],
                },
            )]
            .into();

            // When appliance_kWhcycle_loadingfactor is called
            let (kwh_cycle, loadingfactor) =
                appliance_kwh_cycle_loading_factor(&input, appliance_name, &appliance_map).unwrap();

            // Then adjustment = residual_moisture(F) / 0.6 = 0.90 / 0.6 = 1.5
            assert_eq!(kwh_cycle, 1.5); // 1.0 * (0.90 / 0.6)
            assert_eq!(loadingfactor, 1.2); // 6.0 / 5.0
        }

        #[test]
        fn test_kwh_per_100cycle_is_normalised_to_kwh_per_cycle() {
            // Given an appliance specified in kWh per 100 cycles
            let input = InputForProcessing {
                input: json!({
                    "Appliances": {
                        "Clothes_washing": {
                            "kWh_per_100cycle": 50.0,  // -> 0.5 kWh/cycle
                            "kg_load": 5.0,
                        }
                    }
                }),
            };
            let appliance_name = "Clothes_washing";
            let appliance_map = [(
                "Clothes_washing",
                ApplianceUseProfile {
                    util_unit: 0.0,
                    use_data: Some(ApplianceUseData {
                        use_metric: 0,
                        clothes_use_data: Some(ClothesUseData {
                            standard_load_kg: 6.0,
                        }),
                        _standard_use: None,
                        duration: 0.0,
                        duration_deviation: 0.0,
                    }),
                    standby: 0.0,
                    gains_frac: 0.0,
                    prof: vec![],
                },
            )]
            .into();

            // When appliance_kWhcycle_loadingfactor is called
            let (kwh_cycle, loadingfactor) =
                appliance_kwh_cycle_loading_factor(&input, appliance_name, &appliance_map).unwrap();

            // Then it normalises to kWh/cycle correctly
            assert_eq!(kwh_cycle, 0.5);
            // And loading factor is still applied for laundry appliances
            assert_eq!(loadingfactor, 1.2); // 6.0 / 5.0
        }
    }

    mod combined_schedule_setpoint {
        use super::*;

        #[rstest]
        fn test_unoccupied(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let heating_livingroom = false;
            let heating_restofdwelling = false;
            let setpoint = combined_schedule_setpoint(
                &whole_dwelling_zone,
                temp_setback,
                heating_livingroom,
                heating_restofdwelling,
            )
            .unwrap();
            let expected_setpoint = 18.; // (18 * 25 + 18 * 100) / 125
            assert_eq!(setpoint, expected_setpoint);
        }

        #[rstest]
        fn test_livingroom_only(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let heating_livingroom = true;
            let heating_restofdwelling = false;
            let setpoint = combined_schedule_setpoint(
                &whole_dwelling_zone,
                temp_setback,
                heating_livingroom,
                heating_restofdwelling,
            )
            .unwrap();
            let expected_setpoint = 18.6; // (21 * 25 + 18 * 100) / 125
            assert_eq!(setpoint, expected_setpoint);
        }

        #[rstest]
        fn test_restofdwelling_only(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let heating_livingroom = false;
            let heating_restofdwelling = true;
            let setpoint = combined_schedule_setpoint(
                &whole_dwelling_zone,
                temp_setback,
                heating_livingroom,
                heating_restofdwelling,
            )
            .unwrap();
            let expected_setpoint = 19.6; // (18 * 25 + 20 * 100) / 125
            assert_eq!(setpoint, expected_setpoint);
        }
        #[rstest]
        fn test_both_occupied(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let heating_livingroom = true;
            let heating_restofdwelling = true;
            let setpoint = combined_schedule_setpoint(
                &whole_dwelling_zone,
                temp_setback,
                heating_livingroom,
                heating_restofdwelling,
            )
            .unwrap();
            let expected_setpoint = 20.2; // (21 * 25 + 20 * 100) / 125
            assert_eq!(setpoint, expected_setpoint);
        }
    }

    mod separate_time_and_temp_control_weekday_heating_schedule {
        use super::*;

        #[rstest]
        fn test_unsupported_large_advanced_start(
            whole_dwelling_zone: JsonValue,
            temp_setback: f64,
        ) {
            let advanced_start = 8.;
            let result = separate_time_and_temp_control_weekday_heating_schedule(
                &whole_dwelling_zone,
                temp_setback,
                advanced_start,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .starts_with("advanced_start exceeds 7 hours"),);
        }

        #[rstest]
        fn test_no_advanced_start(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let advanced_start = 0.;
            let schedule = separate_time_and_temp_control_weekday_heating_schedule(
                &whole_dwelling_zone,
                temp_setback,
                advanced_start,
            )
            .unwrap();
            let mut expected_schedule: Vec<Option<f64>> = Vec::with_capacity(48);
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 5]); // both occupied
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(18.6); 4]); // livingroom only
            expected_schedule.extend(vec![Some(20.2); 7]); // both occupied
            expected_schedule.extend(vec![None; 4]); // unoccupied

            assert_eq!(schedule.to_vec(), expected_schedule);
        }

        #[rstest]
        fn test_thirty_min_advanced_start(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let advanced_start = 0.5;
            let schedule = separate_time_and_temp_control_weekday_heating_schedule(
                &whole_dwelling_zone,
                temp_setback,
                advanced_start,
            )
            .unwrap();
            let mut expected_schedule: Vec<Option<f64>> = Vec::with_capacity(48);
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 5]); // both occupied
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(18.6); 3]); // livingroom only
            expected_schedule.extend(vec![Some(20.2)]); // livingroom only, plus restofdwelling advanced start
            expected_schedule.extend(vec![Some(20.2); 7]); // both occupied
            expected_schedule.extend(vec![None; 4]); // unoccupied

            assert_eq!(schedule.to_vec(), expected_schedule);
        }

        #[rstest]
        fn test_one_hour_advanced_start(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let advanced_start = 1.;
            let schedule = separate_time_and_temp_control_weekday_heating_schedule(
                &whole_dwelling_zone,
                temp_setback,
                advanced_start,
            )
            .unwrap();
            let mut expected_schedule: Vec<Option<f64>> = Vec::with_capacity(48);
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 5]); // both occupied
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(18.6); 2]); // livingroom only
            expected_schedule.extend(vec![Some(20.2); 2]); // livingroom only, plus restofdwelling advanced start
            expected_schedule.extend(vec![Some(20.2); 7]); // both occupied
            expected_schedule.extend(vec![None; 4]); // unoccupied

            assert_eq!(schedule.to_vec(), expected_schedule);
        }

        #[rstest]
        fn test_two_hour_advanced_start(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let advanced_start = 2.;
            let schedule = separate_time_and_temp_control_weekday_heating_schedule(
                &whole_dwelling_zone,
                temp_setback,
                advanced_start,
            )
            .unwrap();
            let mut expected_schedule: Vec<Option<f64>> = Vec::with_capacity(48);
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 5]); // both occupied
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 4]); // livingroom only, plus restofdwelling advanced start
            expected_schedule.extend(vec![Some(20.2); 7]); // both occupied
            expected_schedule.extend(vec![None; 4]); // unoccupied

            assert_eq!(schedule.to_vec(), expected_schedule);
        }
    }

    mod separate_temp_control_weekday_heating_schedule {
        use super::*;

        #[rstest]
        fn test_schedule(whole_dwelling_zone: JsonValue) {
            let schedule =
                separate_temp_control_weekday_heating_schedule(&whole_dwelling_zone).unwrap();
            let mut expected_schedule: Vec<Option<f64>> = Vec::with_capacity(48);
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 5]); // both occupied
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 11]); // both occupied
            expected_schedule.extend(vec![None; 4]); // unoccupied

            assert_eq!(schedule.to_vec(), expected_schedule);
        }
    }

    mod weekday_heating_schedule {
        use super::*;

        #[rstest]
        fn test_unknown_heat_control_type(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let advanced_start = 2.;
            let heating_control_type = "IDoNotExistControl";
            let result = weekday_heating_schedule(
                &whole_dwelling_zone,
                temp_setback,
                advanced_start,
                heating_control_type,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .starts_with("Invalid HeatingControlType"))
        }

        #[rstest]
        fn test_separate_temp_control(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let advanced_start = 2.;
            let heating_control_type = "SeparateTempControl";
            let schedule = weekday_heating_schedule(
                &whole_dwelling_zone,
                temp_setback,
                advanced_start,
                heating_control_type,
            )
            .unwrap();

            let mut expected_schedule: Vec<Option<f64>> = Vec::with_capacity(48);
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 5]); // both occupied
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 11]); // both occupied
            expected_schedule.extend(vec![None; 4]); // unoccupied

            assert_eq!(schedule.to_vec(), expected_schedule);
        }

        #[rstest]
        fn test_separate_time_and_temp_control(whole_dwelling_zone: JsonValue, temp_setback: f64) {
            let advanced_start = 1.;
            let heating_control_type = "SeparateTimeAndTempControl";
            let schedule = weekday_heating_schedule(
                &whole_dwelling_zone,
                temp_setback,
                advanced_start,
                heating_control_type,
            )
            .unwrap();

            let mut expected_schedule: Vec<Option<f64>> = Vec::with_capacity(48);
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 5]); // both occupied
            expected_schedule.extend(vec![None; 14]); // unoccupied
            expected_schedule.extend(vec![Some(18.6); 2]); // livingroom only
            expected_schedule.extend(vec![Some(20.2); 2]); // livingroom only, plus restofdwelling advanced start
            expected_schedule.extend(vec![Some(20.2); 7]); // both occupied
            expected_schedule.extend(vec![None; 4]);

            assert_eq!(schedule.to_vec(), expected_schedule);
        }
    }

    mod weekend_heating_schedule {
        use super::*;

        // NB. this test is written in the Python, erroneously, as "test_unknown_heat_control_type"
        #[rstest]
        fn test_schedule(whole_dwelling_zone: JsonValue) {
            let schedule = weekend_heating_schedule(&whole_dwelling_zone).unwrap();

            let mut expected_schedule: Vec<Option<f64>> = Vec::with_capacity(48);
            expected_schedule.extend(vec![None; 17]); // unoccupied
            expected_schedule.extend(vec![Some(20.2); 27]); // both occupied
            expected_schedule.extend(vec![None; 4]); // unoccupied

            assert_eq!(schedule.to_vec(), expected_schedule);
        }
    }

    mod create_heating_pattern {
        use super::*;

        #[fixture]
        fn input() -> InputForProcessing {
            InputForProcessing {
                input: json!({
                    "Control": {},
                    "HeatingControlType": "SeparateTempControl",
                    "HeatSourceWet": {"combi boiler": {"type": "Boiler"}},
                    "SpaceHeatSystem": {"boiler": {"type": "WetDistribution"}},
                    "Zone": {
                        "whole_dwelling": {
                            "livingroom_area": 25.0,
                            "restofdwelling_area": 100.0,
                            "SpaceHeatSystem": "boiler",
                        }
                    },
                }),
            }
        }

        #[rstest]
        fn test_heating_pattern_for_combi_boiler(mut input: InputForProcessing) {
            // Given a dwelling with a combi boiler heating system
            // When create heating pattern is called
            create_heating_pattern(&mut input).unwrap();

            assert_eq!(
                input.input["Control"]["HeatingPattern_boiler"],
                json!({
                    "type": "SetpointTimeControl",
                    "start_day": 0,
                    "time_series_step": 0.5,
                    "schedule": {
                        "main": [{"repeat": 53, "value": "week"}],
                        "week": [{"repeat": 5, "value": "weekday"}, {"repeat": 2, "value": "weekend"}],
                        "weekday": [
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            null,
                            null,
                            null,
                            null,
                        ],
                        "weekend": [
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            null,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            20.2,
                            null,
                            null,
                            null,
                            null,
                        ],
                    },
                    "setpoint_min": 18.0,
                    "advanced_start": 2.0,
                })
            );
            assert_eq!(
                input.input["SpaceHeatSystem"]["boiler"]["Control"],
                "HeatingPattern_boiler"
            );
        }
    }

    mod create_charging_pattern {
        use super::*;

        #[fixture]
        fn input() -> InputForProcessing {
            InputForProcessing {
                input: json!({
                    "Control": {},
                    "HeatingControlType": "SeparateTempControl",
                    "SpaceHeatSystem": {},
                    "Zone": {"whole_dwelling": {"livingroom_area": 25.0, "restofdwelling_area": 100.0}},
                }),
            }
        }

        #[rstest]
        fn test_charging_pattern_for_electric_storage_heater(mut input: InputForProcessing) {
            // Given a project with an electric storage heater as the space heat system
            input.input["SpaceHeatSystem"]["esh"] = json!({"type": "ElecStorageHeater"});
            input.input["Zone"]["whole_dwelling"]["SpaceHeatSystem"] = json!("esh");

            // When the charging pattern is created
            create_charging_pattern(&mut input).unwrap();

            // Then an electric storage heater control charge pattern is added
            let day_schedule = vec![
                true, true, true, true, true, true, true, true, true, true, true, true, true, true,
                false, false, false, false, false, false, false, false, false, false, false, false,
                false, false, false, false, false, false, false, false, false, false, false, false,
                false, false, false, false, false, false, false, false, false, false,
            ];
            assert_eq!(
                input.input["Control"]["ChargingPattern_esh"],
                json!({
                    "type": "ChargeControl",
                    "start_day": 0,
                    "time_series_step": 0.5,
                    "logic_type": "manual",
                    "charge_level": 1,
                    "schedule": {
                        "main": [{"repeat": 365, "value": "day"}],
                        "day": day_schedule,
                    },
                })
            );
        }

        #[rstest]
        fn test_charging_pattern_for_heat_battery(mut input: InputForProcessing) {
            // Given a project with a heat battery as the wet heat source
            input.input["HeatSourceWet"] = json!({"heat_battery": {"type": "HeatBattery"}});

            // When the charging pattern is created
            create_charging_pattern(&mut input).unwrap();

            // Then a HeatBattery control charge pattern is added
            let day_schedule = vec![
                true, true, true, true, true, true, true, true, true, true, true, true, true, true,
                false, false, false, false, false, false, false, false, false, false, false, false,
                false, false, false, false, false, false, false, false, false, false, false, false,
                false, false, false, false, false, false, false, false, false, false,
            ];
            assert_eq!(
                input.input["Control"]["HeatBattery_Control"],
                json!({
                    "type": "ChargeControl",
                    "start_day": 0,
                    "time_series_step": 0.5,
                    "logic_type": "heat_battery",
                    "charge_level": 1,
                    "schedule": {
                        "main": [{"repeat": 365, "value": "day"}],
                        "day": day_schedule,
                    },
                })
            );
            assert_eq!(
                input.input["HeatSourceWet"]["heat_battery"]["ControlCharge"],
                "HeatBattery_Control"
            );
        }
    }

    mod create_space_heat_distribution {
        use super::*;

        #[fixture]
        fn input() -> InputForProcessing {
            InputForProcessing {
                input: json!({
                    // Based on dwelling DESN-H-Det-01
                    "NumberOfHotTappedRooms": 4,
                    "General": {"storeys_in_dwelling": 2},
                    "BuildingLength": 8.004,
                    "BuildingWidth": 6.704,
                    "Zone": {
                        "whole dwelling": {
                            "BuildingElement": {
                                // Three valid walls
                                "wall_1": {
                                    "type": "BuildingElementOpaque",
                                    "base_height": 0,
                                    "height": 2.68,
                                },
                                "wall_2": {
                                    "type": "BuildingElementOpaque",
                                    "base_height": 0,
                                    "height": 2.68,
                                },
                                "wall_3": {
                                    "type": "BuildingElementOpaque",
                                    "base_height": 2.68,
                                    "height": 2.68,
                                },
                                // One unheated roof (should be ignored)
                                "roof": {
                                    "type": "BuildingElementOpaque",
                                    "is_unheated_pitched_roof": true,
                                    "base_height": 2.6,
                                    "height": 3,
                                },
                            }
                        }
                    },
                    "SpaceHeatSystem": {
                        "zone_1": {
                            "Control": "HeatingPattern_LivingRoom",
                            "EnergySupply": "mains elec",
                            "Zone": "zone 1",
                            "frac_convective": 0.95,
                            "rated_power": 6.0,
                            "type": "WetDistribution",
                        },
                        "zone_2": {
                            "Control": "HeatingPattern_RestOfDwelling",
                            "EnergySupply": "mains elec",
                            "Zone": "zone 2",
                            "frac_convective": 0.95,
                            "rated_power": 6.0,
                            "type": "WetDistribution",
                        },
                    },
                }),
            }
        }

        #[rstest]
        fn test_with_example_dwelling(mut input: InputForProcessing) {
            // Given the test dwelling defined above
            // When distribution is created
            create_space_heat_distribution(&mut input).unwrap();
            // Then the result should contain 2 pipe entries
            let distribution = &input.input["SpaceHeatSystem"]["zone_1"]["pipework"];
            assert!(distribution.as_array().is_some_and(|d| d.len() == 2));
            // And each pipe should have either 15mm or 22mm diameter
            let internal_diameters = distribution
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["internal_diameter_mm"].as_f64().unwrap())
                .collect::<Vec<_>>();
            let external_diameters = distribution
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["external_diameter_mm"].as_f64().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(internal_diameters, vec![13., 20.]);
            assert_eq!(external_diameters, vec![15., 22.]);
            // And the pipes should have the expected lengths
            for pipe in distribution.as_array().unwrap() {
                if pipe["internal_diameter_mm"].as_f64().unwrap() == 13. {
                    assert_eq!(pipe["length"].as_f64().unwrap(), 66.21);
                } else {
                    assert_eq!(pipe["length"].as_f64().unwrap(), 23.75);
                }
                // And other values get fixed values
                assert_eq!(pipe["location"].as_str().unwrap(), "internal");
                assert_eq!(pipe["pipe_contents"].as_str().unwrap(), "water");
                assert!(!pipe["surface_reflectivity"].as_bool().unwrap());
                assert_eq!(pipe["insulation_thickness_mm"].as_f64().unwrap(), 0.);
                assert_eq!(
                    pipe["insulation_thermal_conductivity"].as_f64().unwrap(),
                    0.035
                );
            }
        }

        #[rstest]
        fn test_only_wet_distribution_gets_pipework(mut input: InputForProcessing) {
            // When distribution is created with a zone that is of type other than WetDistribution
            input.input["SpaceHeatSystem"]["zone_1"]["type"] = json!("InternalElectric");
            create_space_heat_distribution(&mut input).unwrap();
            // Then the pipework property doesn't exist
            assert!(input.input["SpaceHeatSystem"]["zone_1"]
                .get("pipework")
                .is_none());
        }

        #[rstest]
        fn test_all_zones_get_same_pipework(mut input: InputForProcessing) {
            // When distribution is created with more than one zone in the SpaceHeatSystem
            create_space_heat_distribution(&mut input).unwrap();
            // Then the calculated pipe lengths are as expected
            let expected_distribution = json!([
                {
                    "insulation_thermal_conductivity": 0.035,
                    "insulation_thickness_mm": 0,
                    "external_diameter_mm": 15,
                    "internal_diameter_mm": 13,
                    "location": "internal",
                    "pipe_contents": "water",
                    "surface_reflectivity": false,
                    "length": 66.21,
                },
                {
                    "insulation_thermal_conductivity": 0.035,
                    "insulation_thickness_mm": 0,
                    "external_diameter_mm": 22,
                    "internal_diameter_mm": 20,
                    "location": "internal",
                    "pipe_contents": "water",
                    "surface_reflectivity": false,
                    "length": 23.75,
                },
            ]);
            let zone_1_distribution = input.input["SpaceHeatSystem"]["zone_1"]["pipework"].clone();
            let zone_2_distribution = input.input["SpaceHeatSystem"]["zone_2"]["pipework"].clone();
            assert_eq!(zone_1_distribution, expected_distribution);
            // And each zone gets the same pipework values
            assert_eq!(zone_1_distribution, zone_2_distribution);
        }

        #[rstest]
        fn test_non_zero_base_height(mut input: InputForProcessing) {
            // Given all walls have non-zero base_height
            input.input["Zone"]["whole dwelling"]["BuildingElement"] = json!({
                "wall_1": {"type": "BuildingElementOpaque", "base_height": 2.8, "height": 2.5},
                "wall_2": {"type": "BuildingElementOpaque", "base_height": 2.8, "height": 2.4},
                "wall_3": {"type": "BuildingElementOpaque", "base_height": 6, "height": 2.6},
            });
            // When distribution is created
            create_space_heat_distribution(&mut input).unwrap();
            // Then the calculated pipe lengths are as expected
            let expected_distribution = json!([
                {
                    "insulation_thermal_conductivity": 0.035,
                    "insulation_thickness_mm": 0,
                    "external_diameter_mm": 15,
                    "internal_diameter_mm": 13,
                    "location": "internal",
                    "pipe_contents": "water",
                    "surface_reflectivity": false,
                    "length": 66.81,
                },
                {
                    "insulation_thermal_conductivity": 0.035,
                    "insulation_thickness_mm": 0,
                    "external_diameter_mm": 22,
                    "internal_diameter_mm": 20,
                    "location": "internal",
                    "pipe_contents": "water",
                    "surface_reflectivity": false,
                    "length": 23.75,
                },
            ]);
            let distribution = input.input["SpaceHeatSystem"]["zone_1"]["pipework"].clone();
            assert_eq!(distribution, expected_distribution);
        }

        #[rstest]
        fn test_valid_roof_is_included(mut input: InputForProcessing) {
            // Given two valid walls and one valid roof (not unheated)
            input.input["Zone"]["whole dwelling"]["BuildingElement"] = json!({
                "wall_1": {"type": "BuildingElementOpaque", "base_height": 0, "height": 2.8},
                "wall_2": {"type": "BuildingElementOpaque", "base_height": 0, "height": 2.8},
                "roof_1": {
                    "type": "BuildingElementOpaque",
                    "base_height": 3.0,
                    "height": 3.0,
                    "is_unheated_pitched_roof": false,  // Should be used
                },
            });
            // When distribution is created
            create_space_heat_distribution(&mut input).unwrap();
            // Then the roof element should be used to calculate pipe lengths
            let expected_distribution = json!([
                {
                    "insulation_thermal_conductivity": 0.035,
                    "insulation_thickness_mm": 0,
                    "external_diameter_mm": 15,
                    "internal_diameter_mm": 13,
                    "location": "internal",
                    "pipe_contents": "water",
                    "surface_reflectivity": false,
                    "length": 67.07,
                },
                {
                    "insulation_thermal_conductivity": 0.035,
                    "insulation_thickness_mm": 0,
                    "external_diameter_mm": 22,
                    "internal_diameter_mm": 20,
                    "location": "internal",
                    "pipe_contents": "water",
                    "surface_reflectivity": false,
                    "length": 23.75,
                },
            ]);

            let distribution = input.input["SpaceHeatSystem"]["zone_1"]["pipework"].clone();
            assert_eq!(distribution, expected_distribution);
        }

        #[rstest]
        fn test_different_main_dwelling_properties(mut input: InputForProcessing) {
            // Given different known dwelling types with corresponding known
            // pipework lengths
            let test_cases = [
                json!({
                    "dwelling": "DESN-H-Det-01",
                    "storeys": 2,
                    "length": 8.004,
                    "width": 6.704,
                    "height": 2.68,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 66.21},
                        {"internal_diameter_mm": 20, "length": 23.75},
                    ],
                }),
                json!({
                    "dwelling": "DESN-H-End-02",
                    "storeys": 2,
                    "length": 8.004,
                    "width": 6.606,
                    "height": 2.68,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 65.25},
                        {"internal_diameter_mm": 20, "length": 23.73},
                    ],
                }),
                json!({
                    "dwelling": "DESN-H-Mid-03",
                    "storeys": 2,
                    "length": 8.004,
                    "width": 6.508,
                    "height": 2.68,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 64.28},
                        {"internal_diameter_mm": 20, "length": 23.7},
                    ],
                }),
                json!({
                    "dwelling": "KMHO-H-Det-01",
                    "storeys": 2,
                    "length": 8.345,
                    "width": 6.77,
                    "height": 2.647,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 69.62},
                        {"internal_diameter_mm": 20, "length": 24.53},
                    ],
                }),
                json!({
                    "dwelling": "AECO-F-Gro-01",
                    "storeys": 1,
                    "length": 9.3,
                    "width": 7.37,
                    "height": 2.8,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 42.5},
                        {"internal_diameter_mm": 20, "length": 26.83},
                    ],
                }),
                json!({
                    "dwelling": "AECO-F-Gro-02",
                    "storeys": 1,
                    "length": 6.8,
                    "width": 6.45,
                    "height": 2.8,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 27.19},
                        {"internal_diameter_mm": 20, "length": 21.03},
                    ],
                }),
                json!({
                    "dwelling": "AECO-F-Mid-01",
                    "storeys": 1,
                    "length": 9.3,
                    "width": 7.37,
                    "height": 2.8,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 42.5},
                        {"internal_diameter_mm": 20, "length": 26.83},
                    ],
                }),
                json!({
                    "dwelling": "AECO-F-Mid-02",
                    "storeys": 1,
                    "length": 6.8,
                    "width": 6.45,
                    "height": 2.8,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 27.19},
                        {"internal_diameter_mm": 20, "length": 21.03},
                    ],
                }),
                json!({
                    "dwelling": "AECO-F-Top-01",
                    "storeys": 1,
                    "length": 9.3,
                    "width": 7.37,
                    "height": 2.8,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 42.5},
                        {"internal_diameter_mm": 20, "length": 26.83},
                    ],
                }),
                json!({
                    "dwelling": "AECO-F-Top-02",
                    "storeys": 1,
                    "length": 6.8,
                    "width": 6.45,
                    "height": 2.8,
                    "expected": [
                        {"internal_diameter_mm": 13, "length": 27.19},
                        {"internal_diameter_mm": 20, "length": 21.03},
                    ],
                }),
            ];
            for test_case in test_cases {
                input.input["General"]["storeys_in_dwelling"] = test_case["storeys"].clone();
                input.input["BuildingLength"] = test_case["length"].clone();
                input.input["BuildingWidth"] = test_case["width"].clone();
                input.input["Zone"]["whole dwelling"]["BuildingElement"] = json!({
                    "wall_1": {
                        "type": "BuildingElementOpaque",
                        "base_height": 0,
                        "height": test_case["height"],
                    }
                });
                if test_case["storeys"] == 2 {
                    input.input["Zone"]["whole dwelling"]["BuildingElement"]
                        .as_object_mut()
                        .unwrap()
                        .insert(
                            "wall_2".into(),
                            json!({
                                "type": "BuildingElementOpaque",
                                "base_height": test_case["height"],
                                "height": test_case["height"],
                            }),
                        );
                }
                // When the space heat distribution function is called
                create_space_heat_distribution(&mut input).unwrap();
                // The pipework distribution matches
                let distribution = json!(input.input["SpaceHeatSystem"]["zone_1"]["pipework"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pipe| json!({
                        "length": pipe["length"].clone(),
                        "internal_diameter_mm": pipe["internal_diameter_mm"].clone(),
                    }))
                    .collect_vec());
                assert_eq!(distribution, test_case["expected"]);
            }
        }
    }

    mod calc_sfp_mech_vent {
        use super::*;

        #[fixture]
        fn input() -> InputForProcessing {
            InputForProcessing {
                input: json!({
                    "InfiltrationVentilation": {
                        "MechanicalVentilation": {
                            "mech1": {
                                "EnergySupply": "mains elec",
                                "design_outdoor_air_flow_rate": 111.6,
                                "measured_air_flow_rate": 30,
                                "measured_fan_power": 3,
                                "mid_height_air_flow_path": 5.5,
                                "orientation360": 0,
                                "pitch": 90,
                                "vent_type": "Centralised continuous MEV",
                            }
                        }
                    }
                }),
            }
        }

        #[rstest]
        fn test_sfp_calc_cmev(mut input: InputForProcessing) {
            // Given a dwelling with a cMEV which has a measured_air_flow_rate and measured_fan_power,
            // but no SFP
            // When the SFP is calculated
            calc_sfp_mech_vent(&mut input).unwrap();
            // Then the correct value is returned (3 / 30)
            assert_eq!(
                input.input["InfiltrationVentilation"]["MechanicalVentilation"]["mech1"]["SFP"],
                0.1
            );
        }

        #[rstest]
        fn test_sfp_calc_mvhr(mut input: InputForProcessing) {
            // Given a dwelling with a MVHR which has a measured_air_flow_rate and measured_fan_power,
            // but no SFP
            input.input["InfiltrationVentilation"]["MechanicalVentilation"]["mech1"]["vent_type"] =
                json!("MVHR");
            // When the SFP is calculated
            calc_sfp_mech_vent(&mut input).unwrap();
            // Then the correct value is returned (3 / 30)
            assert_eq!(
                input.input["InfiltrationVentilation"]["MechanicalVentilation"]["mech1"]["SFP"],
                0.1
            );
        }

        #[rstest]
        fn test_input_sfp_retained(mut input: InputForProcessing) {
            // Given a dwelling with a cMEV which has a SFP defined
            input.input["InfiltrationVentilation"]["MechanicalVentilation"]["mech1"]["SFP"] =
                json!(0.5);
            // When the SFP is calculated
            calc_sfp_mech_vent(&mut input).unwrap();
            // Then the input value is retained
            assert_eq!(
                input.input["InfiltrationVentilation"]["MechanicalVentilation"]["mech1"]["SFP"],
                0.5
            );
        }

        #[rstest]
        fn test_input_sfp_retained_with_intermittent(mut input: InputForProcessing) {
            // Given a dwelling with a iMEV which has a SFP defined (as always required by the schema)
            input.input["InfiltrationVentilation"]["MechanicalVentilation"]["mech1"] = json!({
                "vent_type": "Intermittent MEV",
                "SFP": 1.5,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 240,
                "mid_height_air_flow_path": 1.5,
                "orientation360": 90,
                "pitch": 60,
            });
            // When the SFP is calculated
            calc_sfp_mech_vent(&mut input).unwrap();
            // Then the input value is retained
            assert_eq!(
                input.input["InfiltrationVentilation"]["MechanicalVentilation"]["mech1"]["SFP"],
                1.5
            );
        }
    }

    mod create_hot_water_use_pattern {
        use super::*;

        #[fixture]
        fn input() -> InputForProcessing {
            InputForProcessing {
                input: json!({
                    "PartGcompliance": false,
                    "HotWaterSource": {"combi": {"type": "CombiBoiler"}},
                    "ColdWaterSource": {
                        "header tank": {
                            "start_day": 0,
                            "temperatures": [3.0, 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7],
                            "time_series_step": 1,
                        }
                    },
                    "HotWaterDemand": {
                        "Shower": {
                            "mixer": {
                                "type": "MixerShower",
                                "flowrate": 8.0,
                                "ColdWaterSource": "mains water",
                            },
                            "IES": {
                                "type": "InstantElecShower",
                                "rated_power": 9.0,
                                "ColdWaterSource": "mains water",
                                "EnergySupply": "mains elec",
                            },
                        },
                        "Bath": {},
                        "Other": {},
                    },
                    "Events": {},
                }),
            }
        }

        #[rstest]
        fn test_hw_average_applied_to_combiboiler(mut input: InputForProcessing) {
            // Given a dwelling with a combi boiler hot water source
            let tfa = 100.0;
            let n_occupants = 2.;
            let cold_water_feed_temps = vec![10.0; 8760];
            // When the hot water use pattern is created
            create_hot_water_use_pattern(&mut input, tfa, n_occupants, &cold_water_feed_temps)
                .unwrap();
            // Then the expected daily_HW_usage is set on the combi boiler
            // base_hw_usage = 0.70 * 60.3 * (N_occupants ** 0.71)
            // correction_for_missing_elec_showers = 1 + 0.3 * 0.5
            // uplifted_prop_hot_water_showers = 0.60685 * correction_for_missing_elec_showers
            // elec_shower_correction_factor = (1 - 0.60685) + uplifted_prop_hot_water_showers
            // expected = base_hw_usage * elec_shower_correction_factor = 75.33249413626568
            assert_relative_eq!(
                input.input["HotWaterSource"]["combi"]["daily_HW_usage"]
                    .as_f64()
                    .unwrap(),
                75.33249413626568
            );
        }

        #[rstest]
        fn test_hw_average_applied_to_heatpump_hwonly_default(mut input: InputForProcessing) {
            input.input["HotWaterSource"] = json!({
                "tank": {"type": "StorageTank", "HeatSource": {"hp": {"type": "HeatPump_HWOnly"}}}
            });
            let tfa = 100.0;
            let n_occupants = 2.;
            let cold_water_feed_temps = vec![10.0; 8760];
            // When the hot water use pattern is created
            create_hot_water_use_pattern(&mut input, tfa, n_occupants, &cold_water_feed_temps)
                .unwrap();
            // Then the expected vol_hw_daily_average is set on the HW-only heat pump
            // By the same calculations as in test_hw_average_applied_to_combiboiler
            assert_relative_eq!(
                input.input["HotWaterSource"]["tank"]["HeatSource"]["hp"]["vol_hw_daily_average"]
                    .as_f64()
                    .unwrap(),
                75.33249413626568
            );
        }
    }
}
