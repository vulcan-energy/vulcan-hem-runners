use super::future_homes_standard::calc_tfa;
use crate::future_homes_standard::fhs_hw_events::STANDARD_BATH_SIZE;
use crate::future_homes_standard::fhs_part_f_validation::part_f::{
    minimum_background_vent_count_continuous, minimum_background_ventilation_area_continuous,
    minimum_whole_dwelling_ventilation_rate_continuous,
};
use crate::future_homes_standard::fhs_ventilation::{
    create_background_vents, create_mechanical_ventilation,
};
use crate::future_homes_standard::input::InputForProcessing;
use csv::WriterBuilder;
use home_energy_model::output_writer::OutputWriter;
use serde_json::json;

/// This module provides functions to implement pre- and post-processing
/// steps for the Fabric Energy Efficiency calculation run for the Future Homes
/// Standard.
pub fn apply_fhs_fee_preprocessing(input: &mut InputForProcessing) -> anyhow::Result<()> {
    // Calculation assumptions (expressed in comments) are based on SAP 10.2 FEE specification
    //
    // Climate should be same as actual building, but weather data is external
    // to this program so it will have to specified by the user or user interface
    //
    // No heat gain from pumps or fans. Water and space heating systems selected
    // have no pumps or fans, and the only ventilation fans are extract-only so
    // do not lead to heat gains either, so no additional action required for this
    //
    // Window shading should be same as actual building, so no action required here
    //
    // The number of each of the following is the same as the actual building, so
    // no action required here:
    //  - open chimneys
    //  - open flues
    //  - chimneys/flues attached to closed fire
    //  - flues attached to solid fuel boiler
    //  - flues attached to other heater
    //  - blocked chimneys
    //  - passive vents
    //  - flueless gas fires

    // Use continuous decentralised mechanical extract ventilation
    let minimum_air_flow_rate = minimum_whole_dwelling_ventilation_rate_continuous(
        calc_tfa(input)?,
        input.number_of_bedrooms()?,
    );
    let mechanical_ventilation = create_mechanical_ventilation(input, minimum_air_flow_rate)?;
    input.set_mechanical_ventilations(mechanical_ventilation)?;

    // Assign minimum background ventilation
    let minimum_vent_area =
        minimum_background_ventilation_area_continuous(input.number_of_habitable_rooms()?);
    let minimum_vent_count = minimum_background_vent_count_continuous(input.number_of_bedrooms()?);
    let background_vents = create_background_vents(input, minimum_vent_area, minimum_vent_count)?;
    input.set_vents(background_vents)?;

    // Use instantaneous electric water heater
    // Set power such that it should always be sufficient for any realistic demand
    // Look up cold water feed type
    // TODO (from Python) The cold_water_source_name here needs to match the one defined in the
    //                    standard FHS wrapper - ideally these would only be defined in one place
    let cold_water_source_name = if input.cold_water_source_has_header_tank()? {
        "header tank"
    } else {
        "mains water"
    };
    input.set_hot_water_cylinder(json!({
        "type": "PointOfUse",
        "efficiency": 1.0,
        "EnergySupply": "mains elec",
        "ColdWaterSource": cold_water_source_name,
    }))?;
    // Remove any PreHeatedWaterSource (if present) used by the actual building's
    // original "hw cylinder" HotWaterSource
    input.remove_preheated_water_sources()?;
    // No hot water distribution pipework for point of use water heaters
    input.set_water_distribution(json!([]))?;

    // One 9.3 kW InstantElecShower, one bath
    input.set_shower(json!({
        "IES_for_FEE_calc": {
            "type": "InstantElecShower",
            "rated_power": 9.3,
            "EnergySupply": "mains elec",
            "ColdWaterSource": cold_water_source_name,
        }
    }))?;
    input.set_bath(json!({
        "bath for FEE calc": {
            "size": STANDARD_BATH_SIZE,
            "ColdWaterSource": cold_water_source_name,
            "flowrate": 12.0,
        }
    }))?;
    // Other tapping points have 6 litres/min flow rate. This shouldn't make any
    // difference to the space heating/cooling demand, as the number and flowrate
    // of the tapping points is only relevant for distribution losses, which do
    // not apply to point of use water heaters.
    input.set_other_water_use(json!({
        "Other HW for FEE calc": {
            "ColdWaterSource": cold_water_source_name,
            "flowrate": 6,
        }
    }))?;

    // Dwelling achieves water use target of not more than 125 litres/day
    input.set_part_g_compliance(true)?;

    // Remove WWHRS if present
    input.remove_wwhrs()?;

    // Lighting:
    // - capacity same as main FHS wrapper (so will be set in create_lighting_gains function)
    // - efficacy 120 lumens/W
    input.set_lighting_efficacy_for_all_zones(120.0)?;

    // Space heating from InstantElecHeater
    // Set power such that it should always be sufficient for any realistic demand
    // Assume convective fraction for fan heater from BS EN 15316-2:2017 Table B.17
    input.remove_space_heat_systems()?;
    for z_name in input.zone_keys()? {
        let h_name = format!("{z_name}_heating_for_FEE_calc");
        input.set_space_heat_system_for_zone(&z_name, &h_name)?;
        input.set_space_heat_system_for_key(
            &h_name,
            json!({
                "type": "InstantElecHeater",
                "rated_power": 10000.0,
                "convective_type": "Air heating (convectors, fan coils etc.)",
                "EnergySupply": "mains elec",
            }),
        )?;
    }
    // delete any actual HeatSourceWet as it is no longer referenced
    // either by a SpaceHeatSystem or a HotWaterSource under FEE
    input.remove_heat_source_wet()?;

    // Cooling from air conditioning
    // Set capacity such that it should always be sufficient for any realistic demand
    // Efficiency does not matter for this calc so set to 1.0
    // Assume convective fraction for cold air blowing system from BS EN 15316-2:2017 Table B.17
    input.remove_space_cool_systems()?;
    for z_name in input.zone_keys()? {
        let c_name = format!("{z_name}_cooling_for_FEE_calc");
        input.set_space_cool_system_for_zone(&z_name, &c_name)?;
        input.set_space_cool_system_for_key(
            &c_name,
            json!({
                "type": "AirConditioning",
                "cooling_capacity": 10000.0,
                "efficiency": 1.0,
                "frac_convective": 0.95,
                "EnergySupply": "mains elec",
            }),
        )?;
    }

    // Use control type 2 (seperate temperature control but no separate time control)
    input.set_heating_control_type(json!("SeparateTempControl"))?;

    // Remove on-site generation, diverter and electric battery, if present
    input
        .remove_on_site_generation()?
        .remove_all_diverters_from_energy_supplies()?
        .remove_all_batteries_from_energy_supplies()?;

    Ok(())
}

pub fn apply_fhs_fee_postprocessing(
    output_writer: &impl OutputWriter,
    total_floor_area: f64,
    space_heat_demand_total: f64,
    space_cool_demand_total: f64,
    output_mode: &str,
) -> anyhow::Result<()> {
    // Subtract cooling demand from heating demand because cooling demand is negative by convention
    let fabric_energy_eff = calc_fabric_energy_efficiency(
        space_heat_demand_total,
        space_cool_demand_total,
        total_floor_area,
    );
    let location_key = format!("{output_mode}__postproc");
    let writer = output_writer.writer_for_location_key(&location_key, "csv")?;
    let mut writer = WriterBuilder::new().flexible(true).from_writer(writer);

    writer.write_record([
        "Fabric Energy Efficiency",
        "kWh / m2.yr",
        fabric_energy_eff.to_string().as_str(),
    ])?;

    writer.flush()?;

    Ok(())
}

pub(super) fn calc_fabric_energy_efficiency(
    space_heat_demand_total: f64,
    space_cool_demand_total: f64,
    total_floor_area: f64,
) -> f64 {
    (space_heat_demand_total - space_cool_demand_total) / total_floor_area
}
