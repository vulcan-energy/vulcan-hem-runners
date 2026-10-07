use super::future_homes_standard::{
    calc_n_occupants, calc_nbeds, calc_tfa, create_cold_water_feed_temps,
    create_hot_water_use_pattern, create_thermal_penetration, set_temp_internal_static_calcs,
    ENERGY_SUPPLY_NAME_ELECTRICITY, HW_TEMPERATURE, LIVING_ROOM_SETPOINT_FHS,
    REST_OF_DWELLING_SETPOINT_FHS, SIMTIME_END, SIMTIME_START, SIMTIME_STEP,
};
use crate::future_homes_standard::fhs_hw_events::STANDARD_BATH_SIZE;
use crate::future_homes_standard::fhs_part_f_validation::part_f::{
    minimum_background_vent_count_continuous, minimum_background_ventilation_area_continuous,
    minimum_whole_dwelling_ventilation_rate_continuous,
};
use crate::future_homes_standard::fhs_sleeved_dhn_validation::HeatNetworkType;
use crate::future_homes_standard::fhs_ventilation::{
    create_background_vents, create_mechanical_ventilation,
};
use crate::future_homes_standard::input::{
    json_error, InputForProcessing, JsonAccessResult, UValueEditableBuildingElement,
    UValueEditableBuildingElementJsonValue,
};
use anyhow::{anyhow, bail};
use home_energy_model::core::common::WaterSupply;
use home_energy_model::core::energy_supply::energy_supply::EnergySupply;
use home_energy_model::core::energy_supply::energy_supply::EnergySupplyBuilder;
use home_energy_model::core::heating_systems::wwhrs::WwhrsInstantaneous;
use home_energy_model::core::schedule::{expand_events, TypedScheduleEvent};
use home_energy_model::core::space_heat_demand::building_element::{
    pitch_class, HeatFlowDirection,
};
use home_energy_model::core::space_heat_demand::building_element::{R_SE, R_SI_UPWARDS};
use home_energy_model::core::units::{
    convert_profile_to_daily, JOULES_PER_KILOJOULE, JOULES_PER_KILOWATT_HOUR, WATTS_PER_KILOWATT,
};
use home_energy_model::core::water_heat_demand::cold_water_source::ColdWaterSource;
use home_energy_model::core::water_heat_demand::dhw_demand::DomesticHotWaterDemand;
use home_energy_model::core::water_heat_demand::dhw_demand::HotWaterDemandResult;
use home_energy_model::core::water_heat_demand::misc::{water_demand_to_kwh, WaterEventResult};
use home_energy_model::corpus::{calc_htc_hlp, ColdWaterSources, HtcHlpCalculation};
use home_energy_model::corpus::{HotWaterSource, HotWaterSourceBehaviour};
use home_energy_model::hem_core::simulation_time::SimulationTime;
use home_energy_model::hem_core::simulation_time::SimulationTimeIteration;
use home_energy_model::input::{
    CustomEnergySourceFactor, EcoDesignController, GroundBuildingElement,
    GroundBuildingElementJsonValue, WaterDistribution, WaterPipework,
};
use home_energy_model::statistics::{np_interp, percentile};
use indexmap::IndexMap;
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use smartstring::alias::String;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tracing::instrument;

const NOTIONAL_WWHRS: &str = "Notional_Inst_WWHRS";
const NOTIONAL_HIU: &str = "notionalHIU";
const NOTIONAL_HP: &str = "notional_HP";
const HEATING_PATTERN: &str = "HeatingPattern_Null";
const NOTIONAL_HEAT_NETWORK_NAME: &str = "_notional_heat_network";

#[derive(Clone, Debug)]
struct MockHotWaterSource;

impl HotWaterSourceBehaviour for MockHotWaterSource {
    fn get_cold_water_source(&self) -> WaterSupply {
        unreachable!()
    }

    fn demand_hot_water(
        &self,
        _usage_events: Vec<WaterEventResult>,
        _simtime: SimulationTimeIteration,
    ) -> anyhow::Result<f64> {
        unreachable!()
    }

    fn get_temp_hot_water(
        &self,
        volume_required: f64,
        _volume_required_already: f64,
        _simtime: SimulationTimeIteration,
    ) -> anyhow::Result<Vec<(f64, f64)>> {
        Ok(vec![(HW_TEMPERATURE, volume_required)])
    }
}

/// Apply assumptions and pre-processing steps for the Future Homes Standard Notional building
pub(crate) fn apply_fhs_notional_preprocessing(
    input: &mut InputForProcessing,
    custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
    fhs_fee_assumptions: bool,
) -> anyhow::Result<IndexMap<Arc<str>, CustomEnergySourceFactor>> {
    let is_fee = fhs_fee_assumptions;
    // Check if a heat network is present
    let heat_network_type = check_heatnetwork_status(input)?;

    // Determine cold water source
    let cold_water_source = input.cold_water_source_name()?;

    // Determine the TFA
    let total_floor_area = calc_tfa(input)?;

    edit_lighting_efficacy(input)?;
    edit_opaque_adjztu_elements(input)?;
    edit_glazing_for_glazing_limit(input, total_floor_area)?;
    edit_transparent_element(input)?;
    edit_ground_floors(input)?;
    edit_thermal_bridging(input)?;
    edit_party_walls(input)?;

    // modify bath, shower and other dhw characteristics
    edit_bath_shower_other(input)?;

    // add WWHRS if needed (and remove any existing systems)
    remove_wwhrs_if_present(input)?;
    add_wwhrs(input, &cold_water_source, is_fee)?;

    // remove pv diverter or electric battery if present
    remove_pv_diverter_if_present(input)?;
    remove_electric_battery_if_present(input)?;

    // modify ventilation
    let minimum_air_flow_rate = minimum_whole_dwelling_ventilation_rate_continuous(
        total_floor_area,
        input.number_of_bedrooms()?,
    );
    let minimum_vent_area =
        minimum_background_ventilation_area_continuous(input.number_of_habitable_rooms()?);
    let minimum_vent_count = minimum_background_vent_count_continuous(input.number_of_bedrooms()?);
    edit_infiltration_ventilation(
        input,
        minimum_air_flow_rate,
        minimum_vent_area,
        minimum_vent_count,
    )?;

    // edit space heating system
    let custom_energy_supply_factors = edit_space_heating_system(
        input,
        &cold_water_source,
        total_floor_area,
        heat_network_type,
        custom_energy_supply_factors,
        is_fee,
    )?;

    // modify air-conditioning
    edit_space_cool_system(input)?;

    // add solar pv
    add_solar_pv(input, is_fee, total_floor_area)?;

    Ok(custom_energy_supply_factors)
}

fn check_heatnetwork_status(input: &InputForProcessing) -> anyhow::Result<Option<HeatNetworkType>> {
    Ok(input
        .heat_source_wet()?
        .values()
        .find_map(|source| {
            source
                .get("heat_network_type")
                .map(|v| serde_json::from_value(v.clone()))
        })
        .transpose()?)
}

/// Apply notional lighting efficacy
/// efficacy = 120 lm/W
fn edit_lighting_efficacy(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let notional_lighting_efficacy = 120.0;

    for bulb in input.all_bulbs_mut()? {
        bulb.as_object_mut()
            .ok_or_else(|| json_error("Bulb was not an object"))?
            .insert("efficacy".into(), notional_lighting_efficacy.into());
    }

    Ok(())
}

/// Apply Notional infiltration specifications
/// Notional pressure test result at 50Pa = 4 m3/h.m2
/// All passive openings count are set to zero
/// Assigns mechanical ventilation (dMEVs) fans so the count follows the
/// Actual dwelling for decentralised systems. For centralised systems
/// there must be one dMEV per wet room with positions assigned to
/// window or wall building elements
/// Create background vent for each window
fn edit_infiltration_ventilation(
    input: &mut InputForProcessing,
    minimum_air_flow_rate: f64,
    minimum_vent_area: f64,
    minimum_vent_count: usize,
) -> anyhow::Result<()> {
    let test_result = 4.;

    let mechanical_ventilation = create_mechanical_ventilation(input, minimum_air_flow_rate)?;
    let background_vents = create_background_vents(input, minimum_vent_area, minimum_vent_count)?;

    let infiltration_ventilation = input.infiltration_ventilation_node_mut()?;

    let leaks = infiltration_ventilation
        .entry("Leaks")
        .or_insert(json!({}))
        .as_object_mut()
        .ok_or(anyhow::anyhow!("Leaks was expected to be an object"))?;
    leaks.insert("test_pressure".into(), json!("Standard"));
    leaks.insert("test_result".into(), json!(test_result));
    infiltration_ventilation.insert("MechanicalVentilation".into(), mechanical_ventilation);
    infiltration_ventilation.insert("Vents".into(), background_vents);

    Ok(())
}

/// Apply notional u-value (W/m2K) to:
///
/// external elements: walls (0.18), doors (1.0), roofs (0.11), exposed floors (0.13)
/// elements adjacent to unheated space: walls (0.18), ceilings (0.11), floors (0.13)
/// to differentiate external doors from walls, user input: is_external_door
fn edit_opaque_adjztu_elements(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let mut opaque_adjztu_building_elements =
        input.all_opaque_and_adjztu_building_elements_mut_u_values()?;

    for mut building_element in opaque_adjztu_building_elements
        .iter_mut()
        .map(|json_map| UValueEditableBuildingElementJsonValue(json_map))
    {
        let pitch_class = pitch_class(building_element.pitch()?);
        match pitch_class {
            HeatFlowDirection::Downwards => {
                building_element.set_u_value(0.13);
            }
            HeatFlowDirection::Upwards => {
                building_element.set_u_value(0.11);
            }
            HeatFlowDirection::Horizontal => {
                building_element.set_u_value(0.18);

                if building_element.is_opaque() {
                    if let Some(true) = building_element.is_external_door() {
                        building_element.set_u_value(1.0);
                    }
                }
            }
        }
        // remove the r_c input if it was there, as engine would prioritise it over u_value
        building_element.remove_thermal_resistance_construction();
    }

    Ok(())
}

/// For any walls of type BuildingElementPartyWall, adjust the party_wall_cavity_type to
/// filled_sealed. The motivation is to ensure that there is no heat loss through party
/// walls in the notional. This ultimately means that actual unsealed/unfilled party walls
/// are penalised in comparison to the notional version of that wall
fn edit_party_walls(input: &mut InputForProcessing) -> anyhow::Result<()> {
    for building_element in input.all_party_wall_building_elements_mut()? {
        building_element.insert("party_wall_cavity_type".into(), "filled_sealed".into());
        building_element.shift_remove("party_wall_lining_type");
        building_element.shift_remove("thermal_resistance_cavity");
    }

    Ok(())
}

/// Apply notional u-value to windows & glazed doors and rooflights
/// for windows and glazed doors
/// u-value is 1.2
/// for rooflights
/// u-value is 1.7
fn edit_transparent_element(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let mut building_elements = input.all_transparent_building_elements_mut()?;

    for mut building_element in building_elements
        .iter_mut()
        .map(|json_map| UValueEditableBuildingElementJsonValue(json_map))
    {
        let pitch_class = pitch_class(building_element.pitch()?);
        match pitch_class {
            HeatFlowDirection::Upwards => {
                // rooflight
                building_element.set_u_value(1.7);
                building_element.remove_thermal_resistance_construction();
            }
            _ => {
                // if it is not a roof light, it is a glazed door or window
                building_element.set_u_value(1.2);
                building_element.remove_thermal_resistance_construction();
            }
        }
    }

    Ok(())
}

///Split windows/rooflights and walls/roofs into dictionaries.
fn split_glazing_and_walls(
    input: &mut InputForProcessing,
) -> anyhow::Result<(IndexMap<String, Value>, IndexMap<String, Value>)> {
    let mut windows_rooflight: IndexMap<String, Value> = Default::default();
    let mut walls_roofs: IndexMap<String, Value> = Default::default();

    let building_elements = input.all_building_elements()?;

    for (name, building_element) in building_elements {
        match building_element
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "BuildingElementTransparent" => {
                windows_rooflight.insert(String::from(name), building_element.to_owned());
            }
            "BuildingElementOpaque" => {
                walls_roofs.insert(String::from(name), building_element.to_owned());
            }
            _ => continue,
        }
    }

    Ok((windows_rooflight, walls_roofs))
}

///Calculate difference between old  and new glazing area and adjust the glazing areas
fn calculate_area_diff_and_adjust_glazing_area(
    input: &mut InputForProcessing,
    linear_reduction_factor: f64,
    window_rooflight_element: &Value,
    building_element_reference: &str,
) -> anyhow::Result<f64> {
    // expected to be passed a transparent building element value
    let height = window_rooflight_element
        .get("height")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("Height field not found or not a float"))?;
    let width = window_rooflight_element
        .get("width")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("Width field not found or not a float"))?;
    let old_area = height * width;
    let new_height = height * linear_reduction_factor;
    let new_width = width * linear_reduction_factor;

    input.set_numeric_field_for_building_element(
        building_element_reference,
        "height",
        new_height,
    )?;
    input.set_numeric_field_for_building_element(building_element_reference, "width", new_width)?;

    let new_area = new_height * new_width;

    Ok(old_area - new_area)
}

/// Find all walls/roofs with same orientation and pitch as this window/rooflight.
fn find_walls_roofs_with_same_orientation_and_pitch(
    wall_roofs: &[&Value],
    window_rooflight_element: &Value,
) -> anyhow::Result<Vec<usize>> {
    let window_rooflight_orientation = window_rooflight_element
        .get("orientation360")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("Orientation field not found or not a float"))?;
    let window_rooflight_pitch = window_rooflight_element
        .get("pitch")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("Pitch field not found or not a float"))?;

    let mut indices: Vec<usize> = Default::default();

    for (i, el) in wall_roofs.iter().enumerate() {
        if match el.get("type").and_then(Value::as_str).unwrap_or_default() {
            "BuildingElementOpaque" => {
                let orientation = el.get("orientation360").and_then(Value::as_f64);
                let pitch = el.get("pitch").and_then(Value::as_f64);
                Some(window_rooflight_orientation) == orientation
                    && Some(window_rooflight_pitch) == pitch
            }
            _ => false,
        } {
            indices.push(i);
        }
    }

    if indices.is_empty() {
        bail!(
            "There are no walls/roofs with the same orientation and pitch as the window/rooflight"
        );
    }

    Ok(indices)
}

/// Return the u-value for an upwards building element, e.g. rooflight, from the
/// thermal resistance of construction
fn convert_upwards_element_resistance_to_u_value(thermal_resistance_construction: f64) -> f64 {
    // Calculate the surface thermal resistance using constants from hem core based on
    // BS EN ISO 13789:2017, Table 8: Conventional surface heat transfer coefficients
    let thermal_resistance_surface = R_SI_UPWARDS + R_SE;
    let thermal_resistance_total = thermal_resistance_construction + thermal_resistance_surface;

    1. / thermal_resistance_total
}

/// Calculate max glazing area fraction for notional building, adjusted for rooflights
fn calc_max_glazing_area_fraction(
    input: &InputForProcessing,
    total_floor_area: f64,
) -> anyhow::Result<f64> {
    let mut total_rooflight_area = 0.0;
    let mut sum_uval_times_area = 0.0;

    for element in input.all_building_elements()?.values() {
        if element
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            != "BuildingElementTransparent"
        {
            continue;
        }
        let pitch = element
            .get("pitch")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("Failed to parse pitch as number"))?;
        if pitch_class(pitch) != HeatFlowDirection::Upwards {
            continue;
        }

        let height = element
            .get("height")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("Failed to parse height as number"))?;
        let width = element
            .get("width")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("Failed to parse width as number"))?;
        let u_value: Option<f64> = element.get("u_value").and_then(Value::as_f64);

        let rooflight_area = height * width;
        total_rooflight_area += rooflight_area;
        let u_value = match u_value {
            Some(u_value) => u_value,
            _ => {
                let thermal_resistance_construction = element
                    .get("thermal_resistance_construction")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| {
                        anyhow!("Failed to parse thermal_resistance_construction as number")
                    })?;
                convert_upwards_element_resistance_to_u_value(thermal_resistance_construction)
            }
        };

        sum_uval_times_area += rooflight_area * u_value;
    }

    let rooflight_correction_factor = if total_rooflight_area == 0.0 {
        0.0
    } else {
        let average_rooflight_uval = sum_uval_times_area / total_rooflight_area;
        let rooflight_proportion = total_rooflight_area / total_floor_area;
        (rooflight_proportion * (average_rooflight_uval - 1.2) / 1.2).max(0.0)
    };

    Ok(0.25 - rooflight_correction_factor)
}

/// Resize window/rooflight and wall/roofs to meet glazing limits
fn edit_glazing_for_glazing_limit(
    input: &mut InputForProcessing,
    total_floor_area: f64,
) -> anyhow::Result<()> {
    let total_glazing_area: f64 = input
        .all_building_elements()?
        .values()
        .filter(|el| {
            el.get("type")
                .and_then(Value::as_str)
                .is_some_and(|type_str| type_str == "BuildingElementTransparent")
        })
        .map(|el| {
            let height = el.get("height").and_then(Value::as_f64);
            let width = el.get("width").and_then(Value::as_f64);
            if let (Some(height), Some(width)) = (height, width) {
                Ok(height * width)
            } else {
                bail!(
                    "Failed to parse height and width as numbers for transparent element: {:?}",
                    el
                );
            }
        })
        .sum::<Result<f64, _>>()?;

    let max_glazing_area_fraction = calc_max_glazing_area_fraction(input, total_floor_area)?;
    let max_glazing_area = max_glazing_area_fraction * total_floor_area;

    let (windows_rooflight, walls_roofs) = split_glazing_and_walls(input)?;

    if total_glazing_area > max_glazing_area {
        let linear_reduction_factor = (max_glazing_area / total_glazing_area).sqrt();
        // TODO: deal with case where linear_reduction_factor is NaN (sqrt() is NaN if called on a
        //       negative number, max_glazing_area could come back as a negative number from calc_max_glazing_area_fraction
        //       To do this, we may need to capture a sample input that induces this to happen in the Python, and request
        //       upstream for how to deal with this.

        for (building_element_reference, window_rooflight_element) in windows_rooflight {
            let area_diff = calculate_area_diff_and_adjust_glazing_area(
                input,
                linear_reduction_factor,
                &window_rooflight_element,
                &building_element_reference,
            )?;

            let same_orientation_indices = find_walls_roofs_with_same_orientation_and_pitch(
                &walls_roofs.values().collect::<Vec<_>>(),
                &window_rooflight_element,
            )?;

            let wall_roof_area_total = same_orientation_indices
                .iter()
                .filter_map(|i| {
                    let building_element = walls_roofs.values().nth(*i).unwrap();
                    building_element.get("area").and_then(Value::as_f64)
                })
                .sum::<f64>();

            for i in same_orientation_indices.iter() {
                let wall_roof = walls_roofs.values().nth(*i).unwrap();
                if let Some(area) = wall_roof.get("area").and_then(Value::as_f64) {
                    let wall_roof_prop = area / wall_roof_area_total;

                    let new_area = area + area_diff * wall_roof_prop;

                    input.set_numeric_field_for_building_element(
                        &building_element_reference,
                        "area",
                        new_area,
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Apply notional building ground specifications
///
///     u-value = 0.13 W/m2.K
///     thermal resistance of the floor construction,excluding the ground, r_f = 6.12 m2.K/W
///     linear thermal transmittance, psi_wall_floor_junc = 0.16 W/m.K
pub(crate) fn edit_ground_floors(input: &mut InputForProcessing) -> anyhow::Result<()> {
    // TODO (from Python) waiting from MHCLG/DESNZ for clarification if basement floors and basement walls are treated the same

    for mut building_element in input
        .all_ground_building_elements_mut()?
        .into_iter()
        .map(GroundBuildingElementJsonValue)
    {
        building_element.set_u_value(0.13);
        building_element.set_thermal_resistance_floor_construction(6.12);
        building_element.set_psi_wall_floor_junc(0.16);
    }

    Ok(())
}

/// The notional building must follow the same thermal bridges as specified in
/// SAP10.2 Table R2
///
/// TODO (from Python) - how to deal with ThermalBridging when lengths are not specified?
pub(crate) fn edit_thermal_bridging(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let mut thermal_bridging_elements = input.all_thermal_bridging_elements()?;

    for element in thermal_bridging_elements
        .iter_mut()
        .flat_map(|group| group.values_mut())
        .filter_map(|bridging| bridging.as_object_mut())
    {
        let bridge_type: String = element
            .get("type")
            .and_then(|bridge_type| bridge_type.as_str())
            .ok_or_else(|| anyhow!("Thermal bridging type was expected to be set."))?
            .into();
        match bridge_type.as_str() {
            "ThermalBridgePoint" => {
                element.insert("heat_transfer_coeff".into(), json!(0.));
            }
            "ThermalBridgeLinear" => {
                let junction_type = element.get("junction_type").and_then(|junc| junc.as_str()).and_then(|junc| if TABLE_R2.contains_key(junc) { Some(junc) } else { None }).ok_or_else(|| anyhow!("Thermal bridging junction type was expected to be set and one of the values in SAP10.2 Table R2."))?;
                element.insert(
                    "linear_thermal_transmittance".into(),
                    json!(TABLE_R2[junction_type]),
                );
            }
            unknown_type => bail!(
                "Thermal bridging type was expected to be set and either ThermalBridgePoint or ThermalBridgeLinear was expected, but {unknown_type} was found."
            ),
        }
    }

    Ok(())
}

/// Table R2 from SAP10.2
static TABLE_R2: LazyLock<HashMap<&'static str, f64>> = LazyLock::new(|| {
    HashMap::from([
        ("E1", 0.05),
        ("E2", 0.05),
        ("E3", 0.05),
        ("E4", 0.05),
        ("E5", 0.16),
        ("E19", 0.07),
        ("E20", 0.32),
        ("E21", 0.32),
        ("E22", 0.07),
        ("E6", 0.),
        ("E7", 0.07),
        ("E8", 0.),
        ("E9", 0.02),
        ("E23", 0.02),
        ("E10", 0.06),
        ("E24", 0.24),
        ("E11", 0.04),
        ("E12", 0.06),
        ("E13", 0.08),
        ("E14", 0.08),
        ("E15", 0.56),
        ("E16", 0.09),
        ("E17", -0.09),
        ("E18", 0.06),
        ("E25", 0.06),
        ("P1", 0.08),
        ("P6", 0.07),
        ("P2", 0.),
        ("P3", 0.),
        ("P7", 0.16),
        ("P8", 0.24),
        ("P4", 0.12),
        ("P5", 0.08),
        ("R1", 0.08),
        ("R2", 0.06),
        ("R3", 0.08),
        ("R4", 0.08),
        ("R5", 0.04),
        ("R6", 0.06),
        ("R7", 0.04),
        ("R8", 0.06),
        ("R9", 0.04),
        ("R10", 0.08),
        ("R11", 0.08),
    ])
});

///  Apply heat network settings to notional building calculation in project_dict.
fn edit_add_heatnetwork_heating(
    input: &mut InputForProcessing,
    cold_water_source: &str,
    custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
    is_communal: bool,
) -> anyhow::Result<IndexMap<Arc<str>, CustomEnergySourceFactor>> {
    let notional_heat_network = json!(
     {
        NOTIONAL_HIU: {
            "type": "HIU",
            "EnergySupply": NOTIONAL_HEAT_NETWORK_NAME,
            "power_max": 45,
            "HIU_daily_loss": 0.8,
            "building_level_distribution_losses": 62,
        }
    });
    input.set_heat_source_wet(notional_heat_network)?;

    let notional_hot_water_source = json!({
        "hw cylinder": {
            "type": "HIU",
            "ColdWaterSource": cold_water_source,
            "HeatSourceWet": NOTIONAL_HIU,
            }
    });
    input.set_hot_water_source(notional_hot_water_source)?;

    // Remove any PreHeatedWaterSource (if present) used by the actual building's
    // original "hw cylinder" HotWaterSource
    input.remove_preheated_water_sources()?;

    // condense the custom supply factors down to just a notional heat pump with either:
    // Communal: A standardised set of factors
    // Sleeved: The actual set of factors averaged across all actual custom fuels
    let custom_energy_supply_factors = if is_communal {
        [(
            NOTIONAL_HEAT_NETWORK_NAME.into(),
            CustomEnergySourceFactor {
                emissions_factor_kg_co2e_k_wh: 0.033,
                emissions_factor_kg_co2e_k_wh_including_out_of_scope_emissions: 0.033,
                primary_energy_factor_k_wh_k_wh_delivered: 0.75,
            },
        )]
    } else {
        [(
            NOTIONAL_HEAT_NETWORK_NAME.into(),
            CustomEnergySourceFactor {
                emissions_factor_kg_co2e_k_wh: custom_energy_supply_factors
                    .values()
                    .map(|f| f.emissions_factor_kg_co2e_k_wh)
                    .sum::<f64>()
                    / custom_energy_supply_factors.len() as f64,
                emissions_factor_kg_co2e_k_wh_including_out_of_scope_emissions:
                    custom_energy_supply_factors
                        .values()
                        .map(|f| f.emissions_factor_kg_co2e_k_wh_including_out_of_scope_emissions)
                        .sum::<f64>()
                        / custom_energy_supply_factors.len() as f64,
                primary_energy_factor_k_wh_k_wh_delivered: custom_energy_supply_factors
                    .values()
                    .map(|f| f.primary_energy_factor_k_wh_k_wh_delivered)
                    .sum::<f64>()
                    / custom_energy_supply_factors.len() as f64,
            },
        )]
    }
    .into();

    let heat_network_fuel_data = json!({
        "fuel": "custom",
        "is_export_capable": false,
    });

    input.add_energy_supply_for_key(NOTIONAL_HEAT_NETWORK_NAME, heat_network_fuel_data)?;

    Ok(custom_energy_supply_factors)
}

/// Apply heatpump heating to notional building calculation
fn edit_add_heatpump_heating(
    input: &mut InputForProcessing,
    design_capacity_overall: f64,
) -> anyhow::Result<()> {
    let factors_35 = IndexMap::from([
        ("A", 1.00),
        ("B", 0.62),
        ("C", 0.55),
        ("D", 0.47),
        ("F", 1.05),
    ]);
    let factors_55 = IndexMap::from([
        ("A", 0.99),
        ("B", 0.60),
        ("C", 0.49),
        ("D", 0.51),
        ("F", 1.03),
    ]);

    let mut capacity_results_dict_35: IndexMap<&str, f64> = Default::default();
    for (record, factor) in factors_35 {
        let value = round_by_precision(factor * design_capacity_overall, 1e3);
        capacity_results_dict_35.insert(record, value);
    }

    let mut capacity_results_dict_55: IndexMap<&str, f64> = Default::default();
    for (record, factor) in factors_55 {
        let value = round_by_precision(factor * design_capacity_overall, 1e3);
        capacity_results_dict_55.insert(record, value);
    }

    let notional_hp = serde_json::from_value(json!(
     {
        NOTIONAL_HP: {
            "EnergySupply": "mains elec",
            "backup_ctrl_type": "TopUp",
            "min_modulation_rate_35": 0.4,
            "min_modulation_rate_55": 0.4,
            "min_temp_diff_flow_return_for_hp_to_operate": 0,
            "modulating_control": true,
            "power_crankcase_heater": 0.01,
            "power_heating_circ_pump": capacity_results_dict_55["F"] * 0.003,
            "power_max_backup": 3,
            "power_off": 0,
            "power_source_circ_pump": 0.01,
            "power_standby": 0.01,
            "sink_type": "Water",
            "source_type": "OutsideAir",
            "temp_lower_operating_limit": -10,
            "temp_return_feed_max": 60,
            "test_data_EN14825": [
                {
                    "capacity": capacity_results_dict_35["A"],
                    "cop": 2.79,
                    "design_flow_temp": 35,
                    "temp_outlet": 34,
                    "temp_source": -7,
                    "temp_test": -7,
                    "test_letter": "A"
                },
                {
                    "capacity": capacity_results_dict_35["B"],
                    "cop": 4.29,
                    "design_flow_temp": 35,
                    "temp_outlet": 30,
                    "temp_source": 2,
                    "temp_test": 2,
                    "test_letter": "B"
                },
                {
                    "capacity": capacity_results_dict_35["C"],
                    "cop": 5.91,
                    "design_flow_temp": 35,
                    "temp_outlet": 27,
                    "temp_source": 7,
                    "temp_test": 7,
                    "test_letter": "C"
                },
                {
                    "capacity": capacity_results_dict_35["D"],
                    "cop": 8.02,
                    "design_flow_temp": 35,
                    "temp_outlet": 24,
                    "temp_source": 12,
                    "temp_test": 12,
                    "test_letter": "D"
                },
                {
                    "capacity": capacity_results_dict_35["F"],
                    "cop": 2.49,
                    "design_flow_temp": 35,
                    "temp_outlet": 35,
                    "temp_source": -10,
                    "temp_test": -10,
                    "test_letter": "F"
                },
                {
                    "capacity": capacity_results_dict_55["A"],
                    "cop": 2.03,
                    "design_flow_temp": 55,
                    "temp_outlet": 52,
                    "temp_source": -7,
                    "temp_test": -7,
                    "test_letter": "A"
                },
                {
                    "capacity": capacity_results_dict_55["B"],
                    "cop": 3.12,
                    "design_flow_temp": 55,
                    "temp_outlet": 42,
                    "temp_source": 2,
                    "temp_test": 2,
                    "test_letter": "B"
                },
                {
                    "capacity": capacity_results_dict_55["C"],
                    "cop": 4.41,
                    "design_flow_temp": 55,
                    "temp_outlet": 36,
                    "temp_source": 7,
                    "temp_test": 7,
                    "test_letter": "C"
                },
                {
                    "capacity": capacity_results_dict_55["D"],
                    "cop": 6.30,
                    "design_flow_temp": 55,
                    "temp_outlet": 30,
                    "temp_source": 12,
                    "temp_test": 12,
                    "test_letter": "D"
                },
                {
                    "capacity": capacity_results_dict_55["F"],
                    "cop": 1.87,
                    "design_flow_temp": 55,
                    "temp_outlet": 55,
                    "temp_source": -10,
                    "temp_test": -10,
                    "test_letter": "F"
                }
            ],
            "time_constant_onoff_operation": 120,
            "time_delay_backup": 1,
            "type": "HeatPump",
            "var_flow_temp_ctrl_during_test": true
        }
    }))?;

    input.set_heat_source_wet(notional_hp)?;
    Ok(())
}

fn replace_space_heating_system(
    input: &mut InputForProcessing,
    design_capacity: &IndexMap<String, f64>,
    design_flow_temp: f64,
    design_flow_rate: f64,
    temp_diff_emit_dsgn: f64,
    heat_source_name: &str,
    eco_design_controller: &EcoDesignController,
) -> anyhow::Result<()> {
    let n = 1.34;
    let c_per_rad = 1.89 / 50_f64.powf(n);
    let setpoint_for_sizing = LIVING_ROOM_SETPOINT_FHS.max(REST_OF_DWELLING_SETPOINT_FHS);
    let power_output_per_rad = c_per_rad * (design_flow_temp - setpoint_for_sizing).powf(n);
    // thermal mass specified in kJ/K but required in kWh/K
    let thermal_mass_per_rad = 51.8 * JOULES_PER_KILOJOULE as f64 / JOULES_PER_KILOWATT_HOUR as f64;
    // Initialise space heating system in input
    input.remove_space_heat_systems()?;

    for zone_name in input.zone_keys()?.iter() {
        // Calculate number of radiators
        let emitter_cap = *design_capacity
            .get(zone_name)
            .ok_or_else(|| anyhow!("Zone {} not found in design capacity", zone_name))?;
        let number_of_rads = (emitter_cap / power_output_per_rad).ceil();
        // Calculate c and thermal mass
        let c = number_of_rads * c_per_rad;
        let thermal_mass = number_of_rads * thermal_mass_per_rad;
        // Create notional SpaceHeatSystem for the zone
        let zone_space_heat_system_name = format!("{zone_name}_SpaceHeatSystem_Notional");
        input.set_space_heat_system_for_key(&zone_space_heat_system_name, json!({
            "type": "WetDistribution",
            "thermal_mass": thermal_mass,
            "emitters": [{"wet_emitter_type": "radiator", "frac_convective": 0.7, "c": c, "n": n}],
            "temp_diff_emit_dsgn": temp_diff_emit_dsgn,
            "variable_flow": false,
            "HeatSource": {"name": heat_source_name, "temp_flow_limit_upper": 65.0},
            "ecodesign_controller": eco_design_controller,
            "Control": HEATING_PATTERN,
            "design_flow_temp": design_flow_temp,
            "design_flow_rate": design_flow_rate,
            "Zone": zone_name,
            "pipework": [],
        }))?;
        input.set_space_heat_system_for_zone(zone_name, &zone_space_heat_system_name)?;
    }

    Ok(())
}

fn edit_bath_shower_other(input: &mut InputForProcessing) -> anyhow::Result<()> {
    // Bath - standardize flowrate and size
    let notional_bath_flowrate = 12.0; // l/min
    if let Some(baths) = input.baths_mut()? {
        for bath in baths.values_mut() {
            bath.as_object_mut()
                .ok_or_else(|| json_error("Bath was not an object"))?
                .extend([
                    ("flowrate".into(), notional_bath_flowrate.into()),
                    ("size".into(), STANDARD_BATH_SIZE.into()),
                ]);
        }
    };

    // Shower - convert InstantElecShowers to MixerShowers, and standardize flowrate
    let notional_shower_flowrate = 8.; // l/min
    if let Some(shower_values) = input.shower_values_mut()? {
        for shower in shower_values {
            let shower = shower
                .as_object_mut()
                .ok_or_else(|| json_error("Shower was not an object"))?;

            if shower
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "InstantElecShower")
            {
                shower.shift_remove("rated_power");
                shower.shift_remove("EnergySupply");
                shower.insert("type".into(), "MixerShower".into());
            }

            shower.insert("flowrate".into(), notional_shower_flowrate.into());
        }
    }

    // Other - standardize flowrate
    let notional_other_flowrate = 6.0; // l/min
    if let Some(other_water_uses) = input.other_water_uses_mut()? {
        for other in other_water_uses.values_mut() {
            other
                .as_object_mut()
                .ok_or_else(|| json_error("Other water use was not an object"))?
                .insert("flowrate".into(), notional_other_flowrate.into());
        }
    }

    Ok(())
}

fn remove_wwhrs_if_present(input: &mut InputForProcessing) -> anyhow::Result<()> {
    if input.wwhrs()?.is_some() {
        input.remove_wwhrs()?;
    }
    // Remove WWHRS config from any MixerShowers, if present
    for shower in input.shower_values_mut()?.iter_mut().flatten() {
        let shower = shower
            .as_object_mut()
            .ok_or_else(|| json_error("Shower was not an object"))?;
        if shower
            .get("type")
            .and_then(|t| t.as_str())
            .is_some_and(|t| t == "MixerShower")
        {
            shower.remove("WWHRS");
            shower.remove("WWHRS_configuration");
        }
    }

    Ok(())
}

fn add_wwhrs(
    input: &mut InputForProcessing,
    cold_water_source_type: &str,
    is_fee: bool,
) -> anyhow::Result<()> {
    let storeys_in_dwelling = input.storeys_in_dwelling()?;

    // add WWHRS if more than 1 storeys in dwelling and not FEE
    if storeys_in_dwelling > 1 && !is_fee {
        input.register_wwhrs_name_on_showers(NOTIONAL_WWHRS, "B")?;

        input.set_wwhrs(json!({
            NOTIONAL_WWHRS: {
                "ColdWaterSource": cold_water_source_type,
                "system_b_efficiencies": [50, 50],
                "flow_rates": [0.1, 100],
                "type": "WWHRS_Instantaneous",
                "system_b_utilisation_factor": 0.98,
                "system_a_efficiencies": [50, 50],
                "system_a_utilisation_factor": 0.98,
            }
        }))?;
    }

    Ok(())
}

fn calculate_daily_losses(cylinder_vol: f64) -> f64 {
    const CYLINDER_LOSS: f64 = 0.005;
    const FACTORY_INSULATED_THICKNESS_COEFF: f64 = 0.55;
    const THICKNESS: f64 = 120.; // mm

    // calculate cylinder factor insulated factor
    let cylinder_heat_loss_factor =
        CYLINDER_LOSS + FACTORY_INSULATED_THICKNESS_COEFF / (THICKNESS + 4.0);

    // calculate volume factor
    let vol_factor = (120. / cylinder_vol).powf(1. / 3.);

    // Temperature factor
    let temp_factor = 0.6 * 0.9;

    // Calculate daily losses
    cylinder_heat_loss_factor * vol_factor * temp_factor * cylinder_vol
}

fn calc_daily_hw_demand(
    input: &mut InputForProcessing,
    total_floor_area: f64,
    cold_water_source_key: &str,
) -> anyhow::Result<Vec<f64>> {
    // create SimulationTime
    let simtime = SimulationTime::new(SIMTIME_START, SIMTIME_END, SIMTIME_STEP);

    // create ColdWaterSource
    let cold_water_feed_temps = create_cold_water_feed_temps(input)?;
    let cold_water_sources: ColdWaterSources = input
        .cold_water_source()?
        .iter()
        .map(|(key, source)| {
            let key: Arc<str> = Arc::from(key.to_owned());
            (
                key.as_ref().into(),
                Arc::from(ColdWaterSource::new(
                    source.temperatures.clone(),
                    source.start_day,
                    source.time_series_step,
                )),
            )
        })
        .collect();

    let wwhrs: IndexMap<std::string::String, Arc<Mutex<WwhrsInstantaneous>>> = if let Some(
        waste_water_heat_recovery,
    ) =
        input.wwhrs()?
    {
        let notional_wwhrs = waste_water_heat_recovery.get(NOTIONAL_WWHRS).ok_or_else(|| anyhow!("A {} entry for WWHRS was expected to have been set in the FHS Notional wrapper.", NOTIONAL_WWHRS))?;
        [(
            NOTIONAL_WWHRS.to_string(),
            Arc::new(Mutex::new(WwhrsInstantaneous::new(
                notional_wwhrs.flow_rates.clone(),
                notional_wwhrs.system_a_efficiencies.clone(),
                cold_water_sources
                    .get(notional_wwhrs.cold_water_source.as_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "A cold water source could not be found with the type '{:?}'.",
                            notional_wwhrs.cold_water_source
                        )
                    })?
                    .clone(),
                notional_wwhrs.system_a_utilisation_factor,
                notional_wwhrs.system_b_efficiencies.clone(),
                notional_wwhrs.system_b_utilisation_factor,
                None,
                None,
                None,
                None,
            )?)),
        )]
        .into()
    } else {
        Default::default()
    };

    let nbeds = calc_nbeds(input)?;
    let number_of_occupants = calc_n_occupants(total_floor_area, nbeds)?;
    create_hot_water_use_pattern(
        input,
        number_of_occupants,
        total_floor_area,
        &cold_water_feed_temps,
    )?;
    let sim_timestep = simtime.step;
    let total_timesteps = simtime.total_steps();
    let event_types_names_list: Vec<(&str, &str)> = ["Shower", "Bath", "Other"]
        .into_iter()
        .filter_map(|event_type| {
            input
                .hot_water_demand()
                .ok()
                .and_then(|demand| demand.get(event_type))
                .and_then(|events| events.as_object())
                .map(move |events| events.keys().map(move |key| (event_type, key.as_str())))
        })
        .flatten()
        .collect();

    // Initialize a single schedule dictionary
    let mut event_schedules: Vec<Option<Vec<TypedScheduleEvent>>> = vec![None; total_timesteps];

    // Populate the event_schedules dictionary using the modified expand_events function
    for (event_type, event_name) in event_types_names_list {
        let event_data = input.water_heating_event_by_type_and_name(event_type, event_name)?.ok_or_else(|| anyhow!("FHS Notional wrapper expected water heating events with type '{event_type}' and name '{event_name}'"))?.iter().map(Into::into).collect::<Vec<_>>();
        event_schedules = expand_events(
            event_data,
            sim_timestep,
            total_timesteps,
            event_name,
            serde_json::from_value(json!(event_type))?,
            event_schedules,
        )?;
    }

    // Mock hot water source which returns the same temperature at every timestep
    let mock_hw_source: IndexMap<_, _> = IndexMap::from_iter([(
        "hw cylinder".into(),
        HotWaterSource::Fake(Arc::new(MockHotWaterSource)),
    )]);
    let energy_supply: Arc<RwLock<EnergySupply>> = {
        let electricity_supply = input.energy_supply_by_key(ENERGY_SUPPLY_NAME_ELECTRICITY)?;
        Arc::new(RwLock::new(EnergySupplyBuilder::new(
            serde_json::from_value(electricity_supply.and_then(|map| map.get("fuel")).ok_or_else(|| anyhow!("FHS Notional wrapper expected existence of energy supply with key '{ENERGY_SUPPLY_NAME_ELECTRICITY}'"))?.to_owned())?,
            simtime.total_steps(),
        ).with_export_capable(electricity_supply.is_some_and(|map| map.get("is_export_capable").and_then(|v| v.as_bool()).unwrap_or(true))).build()))
    };

    let dhw_demand = DomesticHotWaterDemand::new(
        &input
            .showers()?
            .map(|showers| {
                // we need to remove allow_low_flowrate from all the showers so they can be turned into HEM inputs
                let mut showers = showers.clone();
                for shower in showers.values_mut().filter_map(Value::as_object_mut) {
                    shower.remove("allow_low_flowrate");
                }
                serde_json::from_value(json!(showers))
            })
            .transpose()?
            .unwrap_or_default(),
        &input
            .baths()?
            .map(|baths| serde_json::from_value(json!(baths)))
            .transpose()?
            .unwrap_or_default(),
        &input
            .other_water_uses()?
            .map(|other_water_uses| serde_json::from_value(json!(other_water_uses)))
            .transpose()?
            .unwrap_or_default(),
        &input
            .water_distribution()?
            .unwrap_or(WaterDistribution::List(vec![])),
        &cold_water_sources,
        &wwhrs
            .iter()
            .map(|(key, value)| (key.into(), value.clone()))
            .collect(),
        &IndexMap::from([(String::from("_unmet_demand"), energy_supply)]),
        event_schedules,
        mock_hw_source,
        Default::default(),
    )?;

    // For each timestep, calculate HW draw
    let total_steps = simtime.total_steps();
    let mut hw_energy_demand = vec![0.0; total_steps];
    for (t_idx, t_it) in simtime.iter().enumerate() {
        let HotWaterDemandResult { hw_demand_vol, .. } = dhw_demand.hot_water_demand(t_it)?;

        // Convert from litres to kWh
        let cold_water_temperature =
            cold_water_sources[cold_water_source_key].get_temp_cold_water(0.0, t_it)?;
        hw_energy_demand[t_idx] = water_demand_to_kwh(
            hw_demand_vol["hw cylinder"],
            HW_TEMPERATURE,
            cold_water_temperature[0].0,
        );
    }

    Ok(convert_profile_to_daily(&hw_energy_demand, simtime.step))
}

fn edit_storagetank(
    input: &mut InputForProcessing,
    cold_water_source_type: &str,
    total_floor_area: f64,
) -> anyhow::Result<()> {
    let cylinder_vol = match input.hot_water_cylinder_volume()? {
        Some(volume) => volume,
        None => {
            let daily_hwd = calc_daily_hw_demand(input, total_floor_area, cold_water_source_type)?;
            calculate_cylinder_volume(daily_hwd)
        }
    };

    // Calculate daily losses
    let daily_losses = calculate_daily_losses(cylinder_vol);

    // Modify primary pipework characteristics
    let primary_pipework = edit_primary_pipework(input, total_floor_area)?;

    // Modify cylinder characteristics
    input.set_hot_water_cylinder(json!({
        "ColdWaterSource": cold_water_source_type,
            "HeatSource": {
                NOTIONAL_HP: {
                    "heater_position": 0.1,
                    "name": NOTIONAL_HP,
                    "temp_flow_limit_upper": 60,
                    "thermostat_position": 0.1,
                    "type": "HeatSourceWet"
                }
            },
            "daily_losses": daily_losses,
            "type": "StorageTank",
            "volume": cylinder_vol,
            "primary_pipework": primary_pipework
    }))?;

    // Remove any PreHeatedWaterSource (if present) used by the actual building's
    // original "hw cylinder" HotWaterSource
    input.remove_preheated_water_sources()?;

    Ok(())
}

fn edit_primary_pipework(
    input: &InputForProcessing,
    total_floor_area: f64,
) -> anyhow::Result<Vec<WaterPipework>> {
    // Define minimum values
    let internal_diameter_mm_min = 20.;
    let external_diameter_mm_min = 22.;
    let insulation_thickness_mm_min = 25.;
    let surface_reflectivity = false;
    let pipe_contents = "water";
    let insulation_thermal_conductivity = 0.035;

    let length_max = match input.build_type()?.as_str() {
        "flat" => 0.05 * total_floor_area,
        "house" => {
            0.05 * input.ground_floor_area()?.ok_or_else(|| {
                anyhow!("FHS Notional wrapper expected ground floor area to be set for a house.")
            })?
        }
        unknown_type => bail!("Encountered unexpected building type '{unknown_type}'"),
    };

    let mut primary_pipework = input.primary_pipework_clone()?;

    match primary_pipework {
        None => {
            primary_pipework = Some(vec![serde_json::from_value::<WaterPipework>(json!({
                "location": "internal",
                "internal_diameter_mm": internal_diameter_mm_min,
                "external_diameter_mm": external_diameter_mm_min,
                "length": length_max,
                "insulation_thermal_conductivity": insulation_thermal_conductivity,
                "insulation_thickness_mm": insulation_thickness_mm_min,
                "surface_reflectivity": surface_reflectivity,
                "pipe_contents": pipe_contents
            }))?]);
        }
        Some(ref mut primary_pipework) => {
            for pipework in primary_pipework.iter_mut() {
                let length = pipework.length;
                let internal_diameter_mm =
                    pipework.internal_diameter_mm.max(internal_diameter_mm_min);
                let external_diameter_mm =
                    pipework.external_diameter_mm.max(external_diameter_mm_min);

                // Update insulation thickness based on internal diameter
                let adjusted_insulation_thickness_mm_min = if internal_diameter_mm > 25. {
                    35.
                } else {
                    insulation_thickness_mm_min
                };

                // Primary pipework should not be greater than maximum length
                let length = length.min(length_max);

                // Update pipework
                *pipework = serde_json::from_value(json!({
                    "location": "internal",
                    "internal_diameter_mm": internal_diameter_mm,
                    "external_diameter_mm": external_diameter_mm,
                    "length": length,
                    "insulation_thermal_conductivity": insulation_thermal_conductivity,
                    "insulation_thickness_mm": adjusted_insulation_thickness_mm_min,
                    "surface_reflectivity": surface_reflectivity,
                    "pipe_contents": pipe_contents
                }))?;
            }
        }
    }

    Ok(primary_pipework.unwrap())
}

fn remove_pv_diverter_if_present(
    input: &mut InputForProcessing,
) -> JsonAccessResult<&mut InputForProcessing> {
    input.remove_all_diverters_from_energy_supplies()
}

fn remove_electric_battery_if_present(
    input: &mut InputForProcessing,
) -> JsonAccessResult<&mut InputForProcessing> {
    input.remove_all_batteries_from_energy_supplies()
}

fn edit_space_heating_system(
    input: &mut InputForProcessing,
    cold_water_source: &str,
    total_floor_area: f64,
    heat_network_type: Option<HeatNetworkType>,
    custom_energy_supply_factors: &IndexMap<Arc<str>, CustomEnergySourceFactor>,
    is_fee: bool,
) -> anyhow::Result<IndexMap<Arc<str>, CustomEnergySourceFactor>> {
    // FEE calculation which doesn't need the space heating system at this stage.
    let custom_energy_supply_factors = if !is_fee {
        // If Actual dwelling is heated with heat networks - Notional heated with HIU.
        // Otherwise, notional heated with an air to water heat pump
        let (design_capacity_map, design_capacity_overall) = calc_design_capacity(input)?;

        if let Some(heat_network_type) = heat_network_type {
            let custom_energy_supply_factors = edit_add_heatnetwork_heating(
                input,
                cold_water_source,
                custom_energy_supply_factors,
                heat_network_type == HeatNetworkType::Communal,
            )?;
            let ecodesign_controller: EcoDesignController =
                serde_json::from_value(json!({"ecodesign_control_class": 1}))?;
            replace_space_heating_system(
                input,
                &design_capacity_map,
                55.,
                8.,
                20.,
                NOTIONAL_HIU,
                &ecodesign_controller,
            )?;
            custom_energy_supply_factors
        } else {
            edit_add_heatpump_heating(input, design_capacity_overall)?;
            let ecodesign_controller: EcoDesignController = serde_json::from_value(json!({
                "ecodesign_control_class": 2,
                "max_outdoor_temp": 20,
                "min_flow_temp": 21,
                "min_outdoor_temp": 0,
            }))?;
            replace_space_heating_system(
                input,
                &design_capacity_map,
                45.,
                12.,
                5.,
                NOTIONAL_HP,
                &ecodesign_controller,
            )?;
            edit_storagetank(input, cold_water_source, total_floor_area)?;
            custom_energy_supply_factors.clone()
        }
    } else {
        custom_energy_supply_factors.clone()
    };

    Ok(custom_energy_supply_factors)
}

fn edit_space_cool_system(input: &mut InputForProcessing) -> anyhow::Result<()> {
    let part_o_active_cooling_required = input.part_o_active_cooling_required()?.unwrap_or(false);

    if part_o_active_cooling_required {
        // Update SpaceCoolSystems to have notional values
        input.set_efficiency_for_all_space_cool_systems(5.1)?;
        input.set_frac_convective_for_all_space_cool_systems(0.95)?;
        input.set_energy_supply_for_all_space_cool_systems(ENERGY_SUPPLY_NAME_ELECTRICITY)?;
    } else {
        // Remove all SpaceCoolSystems and all references to them
        input.remove_space_cool_systems()?;
        input.remove_space_cool_systems_for_all_zones()?;
    }

    Ok(())
}

#[instrument(skip(input))]
fn calc_design_capacity(
    input: &InputForProcessing,
) -> anyhow::Result<(IndexMap<String, f64>, f64)> {
    // Create a deep copy as init_resistance_or_uvalue() will add u_value & r_c
    // which will raise warning when called second time
    let mut input = input.clone();

    // Calculate heat transfer coefficients and heat loss parameters
    set_temp_internal_static_calcs(&mut input)?;

    // Override the "Standard"/"Pulse" choice value with the "Standard" test pressure (50)
    input.set_test_pressure_for_infiltration_ventilation_leaks(50.)?;

    // following scope massages the zone JSON values to get them into a shape where they can be successfully deserialised into a HEM ZoneDictionary
    {
        create_thermal_penetration(&mut input)?;
        for zone_key in input.zone_keys()? {
            input.set_init_temp_setpoint_for_zone(&zone_key, 20.)?; // set a temp of 20ºC just to give it a valid value
        }
    }

    let HtcHlpCalculation {
        htc_map: htc_dict, ..
    } = calc_htc_hlp(&input.as_input_for_calc_htc_hlp()?)?;

    // Calculate design capacity
    let min_air_temp = *input.external_conditions()?.air_temperatures().as_ref().ok_or_else(|| anyhow!("FHS Notional wrapper expected to have air temperatures merged onto the input structure."))?.iter().min_by(|a, b| a.total_cmp(b)).ok_or_else(|| anyhow!("FHS Notional wrapper expects air temperature list set on input structure not to be empty."))?;
    let set_point = LIVING_ROOM_SETPOINT_FHS.max(REST_OF_DWELLING_SETPOINT_FHS);
    let temperature_difference = set_point - min_air_temp;
    let design_capacity_map: IndexMap<String, f64> = input
        .zone_keys()?
        .into_iter()
        .map(|key| {
            (key.to_owned(), {
                let design_heat_loss = htc_dict[&key] * temperature_difference;
                let design_capacity = 2. * design_heat_loss;
                design_capacity / WATTS_PER_KILOWATT as f64
            })
        })
        .collect();

    let design_capacity_overall = design_capacity_map.values().sum::<f64>();

    Ok((design_capacity_map, design_capacity_overall))
}

/// Initialise temperature setpoints for all zones.
/// The initial set point is needed to call the Project class.
/// Set as 18C for now. The FHS wrapper will overwrite temp_setpnt_init '''
#[cfg(test)]
fn initialise_temperature_setpoints(input: &mut InputForProcessing) -> anyhow::Result<()> {
    for zone_key in input.zone_keys()? {
        input.set_init_temp_setpoint_for_zone(zone_key.as_str(), 18.)?;
    }
    Ok(())
}

fn add_solar_pv(
    input: &mut InputForProcessing,
    is_fee: bool,
    total_floor_area: f64,
) -> anyhow::Result<()> {
    let build_type = input.build_type()?;

    let storeys_in_building = if build_type == "flat" {
        input
            .storeys_in_building()?
            .ok_or_else(|| anyhow!("expected storeys_in_building for build_type flat"))?
    } else {
        input.storeys_in_dwelling()?
    };

    // PV is included in the notional if the building contains 15 stories or
    // less that contain dwellings.
    if storeys_in_building <= 15 && !is_fee {
        let ground_floor_area = input
            .ground_floor_area()?
            .ok_or_else(|| anyhow!("Notional wrapped expected ground floor area to be set"))?;
        let peak_kw = match input.build_type()?.as_str() {
            "house" => ground_floor_area * 0.4 / 4.5,
            "flat" => total_floor_area * 0.4 / (4.5 * storeys_in_building as f64),
            unknown_type => bail!("Unexpected building type '{unknown_type}' encountered"),
        };

        // If actual has no solar panels then notional will also have nothing
        if let Some(on_site_generation) = input.on_site_generation()? {
            let total_peak_power: f64 = on_site_generation
                .values()
                .filter_map(|panel| panel.get("peak_power").and_then(|power| power.as_f64()))
                .sum();

            // If the calculated peak_kW is less than the actual total power,
            // scale the actual values down to match the peak by applying a scaling factor.
            // The scaling factor is capped at a minimum of 1.0 to avoid scaling up.
            let scaling_factor = 1.0f64.min(peak_kw / total_peak_power);
            let min_inverter_peak_power_kw: f64 = 3.68; // Fixed lower limit on inverter power in kW

            let notional_panels: IndexMap<String, Value> = on_site_generation
                .iter()
                .filter_map(|(name, panel)| {
                    let pv_peak_power = panel
                        .get("peak_power")
                        .and_then(|power| power.as_f64())
                        .unwrap_or(0.0)
                        * scaling_factor;
                    let pv_area = 4.5 * pv_peak_power;
                    // PV width and height based on 2:1 aspect ratio
                    let pv_height = (pv_area / 2.).powf(0.5);
                    let pv_width = 2. * pv_height;
                    let mut updated_panel = panel.clone();
                    let updated_panel = match updated_panel.as_object_mut() {
                        Some(panel) => panel,
                        None => return None,
                    };
                    updated_panel.insert("peak_power".to_string(), json!(pv_peak_power));
                    updated_panel.insert(
                        "inverter_peak_power_ac".to_string(),
                        min_inverter_peak_power_kw
                            .max(
                                panel
                                    .get("inverter_peak_power_ac")
                                    .and_then(|power| power.as_f64())
                                    .unwrap_or(min_inverter_peak_power_kw - 1.),
                            )
                            .into(),
                    ); // default to smaller value than min so always beaten
                    updated_panel.insert(
                        "inverter_peak_power_dc".to_string(),
                        min_inverter_peak_power_kw
                            .max(
                                panel
                                    .get("inverter_peak_power_dc")
                                    .and_then(|power| power.as_f64())
                                    .unwrap_or(min_inverter_peak_power_kw - 1.),
                            )
                            .into(),
                    ); // default to smaller value than min so always beaten
                    updated_panel.insert("height".to_string(), json!(pv_height));
                    updated_panel.insert("width".to_string(), json!(pv_width));

                    Some((String::from(name), json!(updated_panel)))
                })
                .collect();

            input.set_on_site_generation(json!(notional_panels))?;
        }
    }

    Ok(())
}

fn calculate_cylinder_volume(daily_hwd: Vec<f64>) -> f64 {
    // Data from the table
    let percentiles_kwh = [3.7, 4.4, 5.2, 5.9, 6.7, 7.4, 8.1, 8.9, 9.6, 10.3, 11.1];
    let vessel_sizes_litres = [
        165., 190., 215., 240., 265., 290., 315., 340., 365., 390., 415.,
    ];

    // Calculate the 75th percentile of daily hot water demand
    let percentile_75_kwh = percentile(daily_hwd, 75);

    // Use linear interpolation to find the appropriate vessel size
    let interpolated_size_litres =
        np_interp(percentile_75_kwh, &percentiles_kwh, &vessel_sizes_litres);
    let mut interpolated_size_litres = interpolated_size_litres.round();

    // If the size of the hot water storage vessel is unavailable, the next
    // largest size available should be selected
    if !vessel_sizes_litres.contains(&interpolated_size_litres) {
        for size in vessel_sizes_litres {
            if size > interpolated_size_litres {
                interpolated_size_litres = size;
                break;
            }
        }
    }

    interpolated_size_litres
}

fn round_by_precision(src: f64, precision: f64) -> f64 {
    (precision * src).round() / precision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::future_homes_standard::future_homes_standard::create_zone_area;
    use approx::assert_relative_eq;
    use home_energy_model::input::{
        EnergySupplyDetails, HeatSourceWet, HeatSourceWetDetails,
        HotWaterSource as HotWaterSourceInput, HotWaterSourceDetails, SpaceHeatSystemDetails,
    };
    use home_energy_model::input::{PhotovoltaicSystem, WasteWaterHeatRecovery};
    use indexmap::indexmap;
    use rstest::{fixture, rstest};
    use serde_json::json;
    use std::borrow::BorrowMut;
    use std::io::{BufReader, Cursor};

    #[fixture]
    fn test_input() -> InputForProcessing {
        let reader = BufReader::new(Cursor::new(include_str!(
            "./test_assets/fixtures/test_future_homes_standard_notional_input_data.json"
        )));
        let mut input = InputForProcessing::init_with_json_skip_validation(reader).expect(
            "expected valid test_future_homes_standard_notional_input_data.json to be present",
        );

        create_zone_area(&mut input).unwrap();

        // following corrections are because test input data from upstream doesn't match schema
        // remove number field from shading segments
        input.input["ExternalConditions"]["shading_segments"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .for_each(|segment| {
                segment.as_object_mut().unwrap().remove("number");
            });

        // add is_export_capable: false for all energy supplies
        input.input["EnergySupply"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .for_each(|supply| {
                supply["is_export_capable"] = json!(false);
            });

        // add valid Leaks node to InfiltrationVentilation
        input.input["InfiltrationVentilation"]["Leaks"] = json!({
            "ventilation_zone_height": 6,
            "test_pressure": 50,
            "test_result": 1.2,
            "env_area": 220
        });

        // add valid Vents node to InfiltrationVentilation
        input.input["InfiltrationVentilation"]["Vents"] = json!({});

        // add other missing InfiltrationVentilation nodes
        input.input["InfiltrationVentilation"]["altitude"] = json!(30);
        input.input["InfiltrationVentilation"]["cross_vent_possible"] = json!(true);
        input.input["InfiltrationVentilation"]["shield_class"] = json!("Normal");
        input.input["InfiltrationVentilation"]["terrain_class"] = json!("OpenField");

        input
    }

    #[fixture]
    fn tfa(test_input: InputForProcessing) -> f64 {
        calc_tfa(&test_input).unwrap()
    }

    #[fixture]
    fn is_fee() -> bool {
        false
    }

    #[ignore = "currently failing as calc_design_capacity is failing (our test data is missing some expected fields on external conditions)"]
    #[rstest]
    // this test does not exist in Python HEM
    fn test_apply_fhs_notional_preprocessing(mut test_input: InputForProcessing) {
        let fhs_fee_assumptions = false;

        let actual = apply_fhs_notional_preprocessing(
            &mut test_input,
            &Default::default(),
            fhs_fee_assumptions,
        );
        assert!(actual.is_ok())
    }

    #[rstest]
    fn test_edit_lighting_efficacy(mut test_input: InputForProcessing) {
        // Given bulb efficacies not set to 120
        for zone in test_input.input["Zone"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            for bulb in zone["Lighting"]["bulbs"].as_array_mut().unwrap().iter_mut() {
                bulb["efficacy"] = json!(56);
            }
        }

        // When the notional's edit_lighting_efficacy() is called
        edit_lighting_efficacy(&mut test_input).unwrap();

        // Then the efficacy of all bulbs is set to 120
        for zone in test_input.input["Zone"].as_object().unwrap().values() {
            for bulb in zone["Lighting"]["bulbs"].as_array().unwrap().iter() {
                assert_eq!(bulb["efficacy"].as_f64().unwrap(), 120.);
            }
        }
    }

    #[rstest]
    fn test_edit_opaque_ajdztu_elements(mut test_input: InputForProcessing) {
        // Given an example input to the notional FHS
        // When the thermal properties of opaque or adjacent unconditioned elements are set
        edit_opaque_adjztu_elements(&mut test_input).unwrap();

        // Then party walls are not adjusted, using whichever input method is used intact
        let mut whole_dwelling_u_values: IndexMap<String, (Option<f64>, Option<f64>)> =
            indexmap! {};

        for (key, value) in test_input.input["Zone"]["whole dwelling"]["BuildingElement"]
            .as_object()
            .unwrap()
        {
            if ["BuildingElementPartyWall", "BuildingElementOpaque"]
                .contains(&value["type"].as_str().unwrap())
            {
                whole_dwelling_u_values.insert(
                    key.into(),
                    (
                        value
                            .get("thermal_resistance_construction")
                            .and_then(|v| v.as_f64()),
                        value.get("u_value").and_then(|v| v.as_f64()),
                    ),
                );
            }
        }

        let expected: IndexMap<String, (Option<f64>, Option<f64>)> = indexmap! {
            "wall 0".into() => (Some(0.7), None),  // a party wall - thermal resistance left as is
            "wall 1".into() => (None, Some(1.0)),  // non party - door = notional 1
            "wall 2".into() => (None, Some(0.18)),  // non party - horizontal = notional 0.18
            "wall 3".into() => (None, Some(0.3)),  // a party wall- u_value left as is,
            "wall 4".into() => (None, Some(0.11)),  // non party - upwards (ceiling) = notional 0.11
        };

        assert_eq!(whole_dwelling_u_values, expected);
    }

    #[rstest]
    fn test_edit_party_walls_removes_party_wall_lining_type(mut test_input: InputForProcessing) {
        // Given an actual building with an unfilled_sealed party wall
        test_input.input["Zone"]["whole dwelling"]["BuildingElement"]["wall 3"] = json!({
            "type": "BuildingElementPartyWall",
            "u_value": 0.3,
            "areal_heat_capacity": "Very light",
            "mass_distribution_class": "I: Mass concentrated at internal side",
            "pitch": 90,
            "area": 15.0,
            "party_wall_cavity_type": "unfilled_sealed",
            "party_wall_lining_type": "dry_lined",
        });

        // When notional building is created
        edit_party_walls(&mut test_input).unwrap();

        // Then the party wall's party_wall_cavity_type is changed to filled_sealed
        assert_eq!(
            test_input.input["Zone"]["whole dwelling"]["BuildingElement"]["wall 3"]
                ["party_wall_cavity_type"],
            "filled_sealed"
        );
        // And any properties contingent on party_wall_cavity_type are removed
        assert!(
            test_input.input["Zone"]["whole dwelling"]["BuildingElement"]["wall 3"]
                .get("party_wall_lining_type")
                .is_none()
        );
    }

    #[rstest]
    fn test_edit_party_walls_removes_thermal_resistance_cavity(mut test_input: InputForProcessing) {
        // Given an actual building with a defined_resistance party wall
        test_input.input["Zone"]["whole dwelling"]["BuildingElement"]["wall 3"] = json!({"type": "BuildingElementPartyWall",
            "u_value": 0.3,
            "areal_heat_capacity": "Very light",
            "mass_distribution_class": "I: Mass concentrated at internal side",
            "pitch": 90,
            "area": 15.0,
            "party_wall_cavity_type": "defined_resistance",
            "thermal_resistance_cavity": "1.0",});

        // When notional building is created
        edit_party_walls(&mut test_input).unwrap();

        // Then the party wall's party_wall_cavity_type is changed to filled_sealed
        assert_eq!(
            test_input.input["Zone"]["whole dwelling"]["BuildingElement"]["wall 3"]
                ["party_wall_cavity_type"],
            "filled_sealed"
        );
        // And any properties contingent on party_wall_cavity_type are removed
        assert!(
            test_input.input["Zone"]["whole dwelling"]["BuildingElement"]["wall 3"]
                .get("thermal_resistance_cavity")
                .is_none()
        );
    }

    #[rstest]
    fn test_edit_ground_floors(mut test_input: InputForProcessing) {
        let test_input = test_input.borrow_mut();

        edit_ground_floors(test_input).unwrap();

        for zone in test_input.input["Zone"].as_object().unwrap().values() {
            for building_element in zone["BuildingElement"].as_object().unwrap().values() {
                if building_element["type"] == "BuildingElementGround" {
                    assert_eq!(building_element["u_value"], 0.13);
                    assert_eq!(
                        building_element["thermal_resistance_floor_construction"],
                        6.12
                    );
                    assert_eq!(building_element["psi_wall_floor_junc"], 0.16);
                }
            }
        }
    }

    #[rstest]
    fn test_edit_thermal_bridgings(mut test_input: InputForProcessing) {
        let test_input = test_input.borrow_mut();

        edit_thermal_bridging(test_input).unwrap();

        for thermal_bridging in test_input
            .all_thermal_bridgings()
            .unwrap()
            .into_iter()
            .filter_map(|t| t.as_object())
            .flat_map(|v| v.values())
            .filter_map(|v| v.as_object())
        {
            let bridging_type = thermal_bridging.get("type").unwrap().as_str().unwrap();
            if bridging_type == "ThermalBridgePoint" {
                assert_eq!(
                    thermal_bridging
                        .get("heat_transfer_coeff")
                        .unwrap()
                        .as_f64()
                        .unwrap(),
                    0.0
                );
            } else if bridging_type == "ThermalBridgeLinear" {
                let junction_type = thermal_bridging
                    .get("junction_type")
                    .unwrap()
                    .as_str()
                    .unwrap();
                assert!(TABLE_R2.contains_key(junction_type));
                assert_eq!(
                    thermal_bridging
                        .get("linear_thermal_transmittance")
                        .unwrap()
                        .as_f64()
                        .unwrap(),
                    TABLE_R2[junction_type]
                );
            } else {
                panic!("Unknown thermal bridging type '{bridging_type}' encountered");
            }
        }
    }

    #[rstest]
    fn test_calc_max_glazing_area_fraction(mut test_input: InputForProcessing) {
        test_input
            .set_zone(json!({
                "test_zone": {
                    "BuildingElement": {
                        "test_rooflight": {
                            "type": "BuildingElementTransparent",
                            "pitch": 0.0,
                            "height": 2.0,
                            "width": 1.0,
                            "u_value": 1.5, // set field for first assertion
                            // following are necessary fields not mentioned in python fixture
                            "orientation360": 0,
                            "g_value": 1,
                            "frame_area_fraction": 0.5,
                            "base_height": 0,
                            "free_area_height": 2,
                            "mid_height": 1,
                            "max_window_open_area": 0,
                            "window_part_list": [],
                            "shading": [],
                        }
                    }
                }
            }))
            .unwrap();
        assert_eq!(
            calc_max_glazing_area_fraction(&test_input, 80.0).unwrap(),
            0.24375,
            "incorrect max glazing area fraction"
        );
        test_input
            .set_zone(zone_input_for_max_glazing_area_test(1.0, None))
            .unwrap();
        assert_eq!(
            calc_max_glazing_area_fraction(&test_input, 80.0).unwrap(),
            0.25,
            "incorrect max glazing area fraction"
        );
        test_input
            .set_zone(zone_input_for_max_glazing_area_test(1.5, Some(90.)))
            .unwrap();
        assert_eq!(
            calc_max_glazing_area_fraction(&test_input, 80.0).unwrap(),
            0.25,
            "incorrect max glazing area fraction when there are no rooflights"
        );
    }

    fn zone_input_for_max_glazing_area_test(u_value: f64, pitch_override: Option<f64>) -> Value {
        json!({"test_zone": {
            "BuildingElement": {
                "test_rooflight": {
                    "type": "BuildingElementTransparent",
                    "pitch": pitch_override.unwrap_or(0.0),
                    "height": 2.0,
                    "width": 1.0,
                    "u_value": u_value,
                    "orientation360": 0.0,
                    "g_value": 0.0,
                    "frame_area_fraction": 0.0,
                    "free_area_height": 0.0,
                    "base_height": 0.0,
                    "max_window_open_area": 3,
                    "mid_height": 1.5,
                    "shading": [],
                    "window_part_list": []
                }
            },
            "ThermalBridging": 1.0,
            "area": 10.0,
            "volume": 20.0
        }})
    }

    // this test does not exist in Python HEM
    #[rstest]
    fn test_edit_add_heatnetwork_heating(mut test_input: InputForProcessing) {
        let heat_network_name = "_notional_heat_network";

        let _expected_heat_source_wet: HeatSourceWet = serde_json::from_value(json!({
            NOTIONAL_HIU: {
                "type": "HIU",
                "EnergySupply": heat_network_name,
                "power_max": 45,
                "HIU_daily_loss": 0.8,
                "building_level_distribution_losses": 62,
            }
        }))
        .unwrap();

        let expected_hot_water_source: HotWaterSourceInput = serde_json::from_value(json!({
        "hw cylinder": {
            "type": "HIU",
            "ColdWaterSource": "mains water",
            "HeatSourceWet": NOTIONAL_HIU,
            }
        }))
        .unwrap();

        let heat_network_name = "_notional_heat_network";
        let expected_heat_network_fuel_data: EnergySupplyDetails = serde_json::from_value(json!({
            "fuel": "custom",
            "is_export_capable": false,
        }))
        .unwrap();

        edit_add_heatnetwork_heating(&mut test_input, "mains water", &Default::default(), true)
            .unwrap();

        assert_eq!(
            json!(test_input.hot_water_source().unwrap()),
            json!(expected_hot_water_source)
        );

        assert_eq!(
            json!(test_input.energy_supply_by_key(heat_network_name).unwrap()),
            json!(expected_heat_network_fuel_data)
        );
    }

    #[test]
    fn test_convert_upwards_element_resistance_to_u_value() {
        // Given a thermal_resistance_construction of an upwards facing building element of 0.8
        let thermal_resistance_construction = 0.8;

        // When the u-value is calculated
        let u_value =
            convert_upwards_element_resistance_to_u_value(thermal_resistance_construction);

        // Then the value is based on the heat transfer coefficients taken from hem core
        // 1 / (thermal_resistance_construction + R_SI_UPWARDS + R_SE)
        // = 1 / (thermal_resistance_construction + 1 / (H_RI + H_CI_UPWARDS) + 1 / (H_CE + H_RE))
        // = 1 / (0.8 + 1 / (5.13 + 5.0) + 1 / (20.0 + 4.14))
        // = 1.0636694403876181
        let expected_value = 1.0636694403876181;
        assert_relative_eq!(u_value, expected_value);
    }

    #[rstest]
    fn test_edit_bath_shower_other_with_no_shower(mut test_input: InputForProcessing) {
        // Given a building with no shower
        test_input.input["HotWaterDemand"]
            .as_object_mut()
            .unwrap()
            .remove("Shower");

        // When the corresponding notional building is created
        edit_bath_shower_other(&mut test_input).unwrap();

        // Then no notional shower is created
        assert!(test_input.input["HotWaterDemand"].get("Shower").is_none());
    }

    #[rstest]
    fn test_edit_bath_shower_other_with_no_bath(mut test_input: InputForProcessing) {
        // Given a building with no bath
        test_input.input["HotWaterDemand"]
            .as_object_mut()
            .unwrap()
            .remove("Bath");

        // When the corresponding notional building is created
        edit_bath_shower_other(&mut test_input).unwrap();

        // Then no notional bath is created
        assert!(test_input.input["HotWaterDemand"].get("Bath").is_none());
    }

    #[rstest]
    fn test_edit_bath_shower_other_converts_instant_elec_shower(
        mut test_input: InputForProcessing,
    ) {
        // Given an actual building with multiple Instant Electric Showers
        test_input.input["HotWaterDemand"]["Shower"] = json!({
            "main": {
                "type": "InstantElecShower",
                "rated_power": 9.0,
                "ColdWaterSource": "mains water",
                "EnergySupply": "mains elec",
            },
            "ensuite": {
                "type": "InstantElecShower",
                "rated_power": 12.0,
                "ColdWaterSource": "mains water",
                "EnergySupply": "mains elec",
            }
        });

        // When the corresponding notional building is created
        edit_bath_shower_other(&mut test_input).unwrap();

        // Then the showers are converted to a MixerShower with the standard
        // flowrate of 8.0 l/min
        let notional_shower_flowrate = 8.;
        let expected_shower = json!({
            "main": {
                "type": "MixerShower",
                "flowrate": notional_shower_flowrate,
                "ColdWaterSource": "mains water",
            },
            "ensuite": {
                "type": "MixerShower",
                "flowrate": notional_shower_flowrate,
                "ColdWaterSource": "mains water",
            },
        });

        assert_eq!(
            test_input.input["HotWaterDemand"]["Shower"],
            expected_shower
        );
    }

    #[rstest]
    fn test_edit_bath_shower_other_standardises_shower_flowrate(
        mut test_input: InputForProcessing,
    ) {
        // Given an actual building with multiple MixerShowers with non-standard flowrates
        test_input.input["HotWaterDemand"]["Shower"] = json!({"main": {"type": "MixerShower", "flowrate": 14.0, "ColdWaterSource": "mains water"},
            "ensuite": {"type": "MixerShower", "flowrate": 13.0, "ColdWaterSource": "mains water"},
        });

        // When the corresponding notional building is created
        edit_bath_shower_other(&mut test_input).unwrap();

        // Then the showers are updated to have a flowrate of 8.0 l/min
        let notional_shower_flowrate = 8.;
        let expected_shower = json!({
            "main": {
                "type": "MixerShower",
                "flowrate": notional_shower_flowrate,
                "ColdWaterSource": "mains water",
            },
            "ensuite": {
                "type": "MixerShower",
                "flowrate": notional_shower_flowrate,
                "ColdWaterSource": "mains water",
            },
        });

        assert_eq!(
            test_input.input["HotWaterDemand"]["Shower"],
            expected_shower
        );
    }

    #[rstest]
    fn test_edit_bath_shower_other_standardises_other_flowrate(mut test_input: InputForProcessing) {
        // Given an actual building with other hot water demand with non-standard flowrates
        test_input.input["HotWaterDemand"]["Other"] = json!({"kitchen": {"ColdWaterSource": "mains water", "flowrate": 15.0},
            "utility": {"ColdWaterSource": "mains water", "flowrate": 8.0},});

        // When the corresponding notional building is created
        edit_bath_shower_other(&mut test_input).unwrap();

        // Then the flowrate of each object is set to a standard value of 6.0 l/min
        let notional_other_flowrate = 6.;
        let expected_other = json!({
            "kitchen": {"ColdWaterSource": "mains water", "flowrate": notional_other_flowrate},
            "utility": {"ColdWaterSource": "mains water", "flowrate": notional_other_flowrate},
        });

        assert_eq!(test_input.input["HotWaterDemand"]["Other"], expected_other);
    }

    #[rstest]
    fn test_edit_bath_shower_other_standardises_bath_size_and_flowrate(
        mut test_input: InputForProcessing,
    ) {
        // Given an actual building with baths with non-standard sizes and flowrates
        test_input.input["HotWaterDemand"]["Bath"] = json!({
            "main": {"ColdWaterSource": "mains water", "flowrate": 15.0, "size": 200.0},
            "ensuite": {"ColdWaterSource": "mains water", "flowrate": 8.0, "size": 150.0},
        });

        // When the corresponding notional building is created
        edit_bath_shower_other(&mut test_input).unwrap();

        // Then the baths are set to a size of 180mm and a flowrate of 12.0 l/min
        let notional_bath_flowrate = 12.;
        let notional_bath_size = 180.;
        let expected_bath = json!({
            "main": {
                "ColdWaterSource": "mains water",
                "flowrate": notional_bath_flowrate,
                "size": notional_bath_size,
            },
            "ensuite": {
                "ColdWaterSource": "mains water",
                "flowrate": notional_bath_flowrate,
                "size": notional_bath_size,
            },
        });

        assert_eq!(test_input.input["HotWaterDemand"]["Bath"], expected_bath);
    }

    #[rstest]
    fn test_remove_wwhrs_if_present(mut test_input: InputForProcessing) {
        test_input
            .set_wwhrs(json!({
                "main": {
                    "type": "WWHRS_Instantaneous",
                    "ColdWaterSource": "header tank",
                    "flow_rates": [5, 7, 9, 11, 13],
                    "system_a_efficiencies": [63, 54.9, 48.6, 43.6, 39.6],
                    "system_a_utilisation_factor": 0.972,
                }
            }))
            .unwrap();
        test_input
            .set_shower(json!({
            "main": {
                "type": "MixerShower",
                "flowrate": 8.0,
                "ColdWaterSource": "mains water",
                "WWHRS": "main",
                "WWHRS_configuration": "A",
            }}))
            .unwrap();

        // When remove_wwhrs_if_present() is called
        remove_wwhrs_if_present(&mut test_input).unwrap();

        // Then the WWHRS is removed along with references to it in the Showers
        assert!(test_input.wwhrs().unwrap().is_none());
        let main_shower = test_input.showers().unwrap().unwrap().get("main").unwrap();
        assert!(main_shower.get("WWHRS").is_none());
        assert!(main_shower.get("WWHRS_Configuration").is_none());
    }

    #[rstest]
    fn test_remove_wwhrs_if_present_with_no_wwhrs(mut test_input: InputForProcessing) {
        test_input.remove_wwhrs().unwrap();
        let main_shower = json!({
        "main": {
            "type": "MixerShower",
            "flowrate": 8.0,
            "ColdWaterSource": "mains water"
        }});
        test_input.set_shower(main_shower.clone()).unwrap();

        // When remove_wwhrs_if_present() is called
        remove_wwhrs_if_present(&mut test_input).unwrap();

        // Then it has no impact
        assert!(test_input.wwhrs().unwrap().is_none());
        assert_eq!(
            test_input.showers().unwrap().unwrap(),
            main_shower.as_object().unwrap()
        );
    }

    #[rstest]
    fn test_add_wwhrs_adds_wwhrs_config_for_multistory_non_fee(mut test_input: InputForProcessing) {
        // Given an actual building with more that one story and multiple showers
        test_input.set_storeys_in_dwelling(2).unwrap();
        test_input
            .set_shower(json!({
            "main": {
                "type": "MixerShower",
                "flowrate": 8.0,
                "ColdWaterSource": "mains water",
                "WWHRS": "main",
                "WWHRS_configuration": "A",
            },
            "ensuite": {"type": "MixerShower", "flowrate": 8.0, "ColdWaterSource": "mains water"},
            }))
            .unwrap();

        let cold_water_source_type = "mains water";

        // When the notional building's WWHRS is configured
        add_wwhrs(&mut test_input, cold_water_source_type, false).unwrap();

        // Then a WWHRS is added and the showers are updated to use it
        let expected_wwhrs: WasteWaterHeatRecovery = serde_json::from_value(json!({
            "Notional_Inst_WWHRS": {
               "ColdWaterSource": cold_water_source_type,
               "system_b_efficiencies": [50, 50],
               "flow_rates": [0.1, 100],
               "type": "WWHRS_Instantaneous",
               "system_b_utilisation_factor": 0.98,
               "system_a_efficiencies": [50, 50],
               "system_a_utilisation_factor": 0.98,
           }
        }))
        .unwrap();
        let actual_wwhrs = test_input.wwhrs().unwrap();
        assert_eq!(actual_wwhrs.unwrap(), expected_wwhrs);

        for shower_name in ["main", "ensuite"] {
            let shower = &test_input.input["HotWaterDemand"]["Shower"][shower_name];
            assert_eq!(shower["WWHRS"], "Notional_Inst_WWHRS");
            assert_eq!(shower["WWHRS_configuration"], "B");
        }
    }

    #[rstest]
    fn test_add_wwhrs_does_not_add_wwhrs_config_for_single_storey(
        mut test_input: InputForProcessing,
    ) {
        // Given an actual building with one story and multiple showers
        test_input.set_storeys_in_dwelling(1).unwrap();
        test_input
            .set_shower(json!({
            "main": {
                "type": "MixerShower",
                "flowrate": 8.0,
                "ColdWaterSource": "mains water",
                "WWHRS": "main",
                "WWHRS_configuration": "A",
            },
            "ensuite": {"type": "MixerShower", "flowrate": 8.0, "ColdWaterSource": "mains water"},
            }))
            .unwrap();

        let cold_water_source_type = "mains water";

        // When the notional building's WWHRS is configured
        add_wwhrs(&mut test_input, cold_water_source_type, false).unwrap();

        // Then a WWHRS is not added and the showers are not updated to use it
        let showers = &test_input.input["HotWaterDemand"]["Shower"];
        assert_eq!(showers["main"]["WWHRS"], "main");
        assert_eq!(showers["main"]["WWHRS_configuration"], "A");
        assert!(showers["ensuite"].get("WWHRS").is_none());
        assert!(showers["ensuite"].get("WWHRS_configuration").is_none());
        assert!(test_input.wwhrs().unwrap().is_none());
    }

    #[rstest]
    fn test_add_wwhrs_does_not_add_wwhrs_config_for_fee(mut test_input: InputForProcessing) {
        // Given an actual building with more than one story and multiple showers
        test_input.set_storeys_in_dwelling(2).unwrap();
        test_input
            .set_shower(json!({
            "main": {
                "type": "MixerShower",
                "flowrate": 8.0,
                "ColdWaterSource": "mains water",
                "WWHRS": "main",
                "WWHRS_configuration": "A",
            },
            "ensuite": {"type": "MixerShower", "flowrate": 8.0, "ColdWaterSource": "mains water"},
            }))
            .unwrap();

        let cold_water_source_type = "mains water";

        // When the notional building's WWHRS is configured with is_FEE True
        add_wwhrs(&mut test_input, cold_water_source_type, true).unwrap();

        // Then a WWHRS is not added and the showers are not updated to use it
        let showers = &test_input.input["HotWaterDemand"]["Shower"];
        assert_eq!(showers["main"]["WWHRS"], "main");
        assert_eq!(showers["main"]["WWHRS_configuration"], "A");
        assert!(showers["ensuite"].get("WWHRS").is_none());
        assert!(showers["ensuite"].get("WWHRS_configuration").is_none());
    }

    #[rstest]
    fn test_calculate_daily_losses() {
        let cylinder_vol = 265.;
        let actual_daily_losses = calculate_daily_losses(cylinder_vol);
        let expected_daily_losses = 1.03685;
        assert_relative_eq!(
            actual_daily_losses,
            expected_daily_losses,
            max_relative = 1E-6
        );
    }

    #[rstest]
    fn test_edit_storagetank(mut test_input: InputForProcessing) {
        let cold_water_source_type = "mains water";
        let total_floor_area = calc_tfa(&test_input).unwrap();

        edit_storagetank(&mut test_input, cold_water_source_type, total_floor_area).unwrap();

        let expected_primary_pipework = json!([{
            "location": "internal",
            "internal_diameter_mm": 26.,
                "external_diameter_mm": 28.,
                "length": 2.5,
                "insulation_thermal_conductivity": 0.035,
                "insulation_thickness_mm": 35.,
                "surface_reflectivity": false,
                "pipe_contents": "water"
        }]);

        let expected_hotwater_source = json!({
            "hw cylinder": {
                "ColdWaterSource": cold_water_source_type,
                "HeatSource": {
                    "notional_HP": {
                        "heater_position": 0.1,
                        "name": "notional_HP",
                        "temp_flow_limit_upper": 60,
                        "thermostat_position": 0.1,
                        "type": "HeatSourceWet"
                    }
                },
                "daily_losses": 0.46660029577109363,
                "type": "StorageTank",
                "volume": 80.0,
                "primary_pipework": expected_primary_pipework,
            }
        })
        .as_object()
        .unwrap()
        .clone();

        assert_eq!(
            test_input.hot_water_source().unwrap().clone(),
            expected_hotwater_source
        );
    }

    #[rstest]
    #[ignore = "differs as hot water event generation is different due to inability to replicate random seed from Python"]
    fn test_calc_daily_hw_demand(mut test_input: InputForProcessing, tfa: f64) {
        let cold_water_source_type = "mains water";

        // Add notional objects that affect HW demand calc
        edit_bath_shower_other(&mut test_input).unwrap();

        let daily_hwd = calc_daily_hw_demand(&mut test_input, tfa, cold_water_source_type);

        let expected_result = [
            4.494866624219392,
            3.6262929661140406,
            2.4792292219363525,
            7.396905949799487,
            2.334290833000372,
            8.938831222369114,
            4.218109245384848,
            3.2095837032274623,
            1.562913391249543,
            8.846662366836481,
            2.573896298797947,
            3.8158857819823955,
            1.8643761342051466,
            1.456499804780102,
            7.426850861522029,
            2.9833503486722512,
            4.217424343066319,
            8.086072907696455,
            4.14341306475027,
            5.363210797769194,
            4.51254160486555,
            4.535867190456099,
            2.7857687977141605,
            1.7560127175725972,
            10.333211623720878,
            2.0533256568949536,
            10.5846961515653,
            2.9116693757992294,
            6.246398935042146,
            1.6696701053184573,
            13.851788339322859,
            3.492087111231953,
            7.271874351886643,
            3.4529488454587005,
            12.374228697052182,
            4.392672154556575,
            1.6028771659496917,
            2.5058963927074895,
            2.075293843606148,
            2.949279475580221,
            8.392209203216268,
            7.314951072027724,
            3.8238937613049946,
            6.812712493984371,
            4.537127728764957,
            6.858233389853893,
            3.994128161632102,
            6.612136728233484,
            10.073004625703325,
            7.1389148991972755,
            1.5377879632668527,
            2.5192092256423533,
            3.5974699592273436,
            2.677722497971631,
            6.641600203287786,
            2.108183420048377,
            2.0324151238352606,
            4.5025717651118,
            1.6287189927962715,
            5.184027724364109,
            2.19733248287286,
            5.538684171666794,
            1.6808281306652284,
            5.413255340871867,
            2.5025554646129446,
            6.9674570352988034,
            4.018149967311069,
            3.598667197534451,
            2.2197290514730836,
            6.818451857176455,
            5.796189225222955,
            7.768257372957939,
            1.9635622695280748,
            4.990639078067053,
            11.326110200617702,
            7.793122367331328,
            4.50364508936643,
            8.379833734745256,
            3.308002750963755,
            5.125036944678628,
            5.130855988470622,
            6.976241946425853,
            9.280525199389762,
            6.879123493336726,
            3.7542978536142275,
            1.0782890932651108,
            6.663709522738787,
            5.7746999922120015,
            3.9743519683699224,
            9.8172995461514,
            2.9545593496596627,
            5.318321839987381,
            3.3213819919472374,
            7.238785487773112,
            0.975438526348773,
            6.4976602476390495,
            3.7068430878370746,
            3.6626428865004845,
            3.448702673630278,
            2.836638476910602,
            4.504302459687092,
            4.594078645382666,
            1.280400852785038,
            9.635660153417774,
            2.3614201923456397,
            5.013620232473153,
            5.308539972801421,
            3.238066845393417,
            6.598303437213936,
            2.659608088311913,
            3.4249044366870596,
            1.7403514603158758,
            9.599864960640643,
            4.369113075109336,
            3.8042018874949823,
            4.28862554376783,
            8.382412615836817,
            3.6774875962929796,
            9.929521288784244,
            5.062516904173654,
            1.295233711655901,
            3.821798499477692,
            2.7132360178922594,
            4.1887507596892,
            3.0863014076672695,
            5.419182763235196,
            2.4073147138874753,
            4.213814051467208,
            1.4251763271057125,
            4.63864991810561,
            1.8216774464333805,
            7.563505390005953,
            3.555241721557862,
            4.493983266747359,
            3.6876604931200268,
            2.454316031896153,
            6.607387606094413,
            9.661565220506915,
            4.386150963483148,
            8.17494299730526,
            6.3198003788420865,
            11.467482765051136,
            4.4065433247605155,
            4.988557708764494,
            9.059519529597852,
            3.3755152852188606,
            7.698743531592137,
            6.618981677978387,
            4.175477039072181,
            6.806814349660336,
            7.431397132641212,
            3.2631899144907384,
            5.0699911933702815,
            2.544651729021151,
            6.080829912290311,
            3.258481663966277,
            3.2938506150971927,
            1.8260826000310022,
            2.7299288206743455,
            6.385202523388037,
            7.33598893338676,
            6.840098800550557,
            2.4629260392399206,
            2.822974223355313,
            4.03397696765668,
            7.1488374756688975,
            3.5278223212553437,
            1.6660138380568987,
            3.458555531243357,
            4.197013547018917,
            4.16975870859787,
            5.562332837781567,
            5.725468422378183,
            6.185468819167943,
            1.6837093971173656,
            4.104228396783036,
            2.1451522407332986,
            5.200237362413139,
            2.084669219978378,
            2.143187834435002,
            2.3140330225844843,
            2.5126535024521788,
            4.292203829906608,
            6.2948386960261375,
            2.707084807447703,
            1.430079063200245,
            3.1398877317179585,
            4.624382313637398,
            2.1098013499095423,
            3.9693834315834158,
            2.918849367120602,
            1.7223877188894419,
            4.2052444383809435,
            1.7027379387189652,
            1.7409058342821224,
            9.73885109912091,
            3.8320864374919834,
            2.8551701405557335,
            3.3985085530029173,
            2.6417203955118143,
            3.1546804210730217,
            3.4473648609972964,
            4.3394904975655955,
            1.8618334598784554,
            1.187119680626635,
            3.3999014822138482,
            2.4231306986854895,
            4.0855931662787555,
            1.1110467622212452,
            3.0836645479450717,
            5.041340866799629,
            2.8584761392149347,
            3.7750089454569724,
            3.98317291735132,
            1.795715129859829,
            2.627605288805403,
            5.475886512190622,
            3.4271418225056154,
            2.891347603259713,
            5.552587133534232,
            5.9809436633734885,
            2.767071076874223,
            4.710760448075293,
            2.376717698170292,
            4.942802828102577,
            4.892581657820345,
            2.6791926869893503,
            2.4743683782040664,
            1.7379083377994877,
            5.778130144567433,
            7.089071071443536,
            2.0388630666174756,
            3.7782560363505686,
            2.0730543304536373,
            2.1948457120426,
            3.361267582386128,
            4.2629464057701245,
            5.958719480369529,
            3.843708413984395,
            3.4815545720306273,
            8.026655051714712,
            6.732224042552772,
            1.4786422506253278,
            3.359516228956052,
            5.051731271772764,
            10.37713698283845,
            1.5329362087999223,
            6.88186935703012,
            2.2867563460747355,
            3.5695696056108077,
            3.930672254899223,
            3.0399623738750345,
            3.209364172407534,
            3.038123333541644,
            1.7884030890335403,
            6.617270158127451,
            4.7892852624442686,
            7.98246376739204,
            8.787993619430916,
            5.4370125075956715,
            5.631570198072755,
            3.7085584331780943,
            1.5882618579464969,
            6.8903268532947735,
            7.892748258525165,
            4.307146684097607,
            5.325141240693638,
            5.615893606452018,
            3.382501768861289,
            2.341364633783292,
            5.952009533729282,
            3.9511068446824225,
            1.878750671506351,
            8.770931877395236,
            7.139918473168391,
            2.968787917613602,
            6.133155615519703,
            4.665146442297377,
            3.5212090189137006,
            4.272327030053521,
            1.8181271956714553,
            1.2111424719202177,
            3.8362418637305393,
            1.6897828694837667,
            4.081067466294491,
            4.733604465939571,
            2.796815803783686,
            3.542465234414504,
            0.9548600743010305,
            2.270717485512143,
            3.850180844042854,
            3.7517662603259643,
            2.9551810686059867,
            5.147502087772008,
            2.4467917467578144,
            5.105007513097308,
            8.408655228616226,
            4.452315949253354,
            1.6886214201468253,
            3.2675270705264667,
            5.874045148360757,
            3.0273104135176405,
            3.7648099268073,
            8.321357616729175,
            6.922623016214074,
            1.678742522381662,
            2.631473336660425,
            1.9769260252425107,
            8.54513049934888,
            5.606712381312642,
            9.985899928272206,
            7.7550045545739374,
            2.5269986968302973,
            8.642277130729743,
            3.817375807234058,
            5.305481369727255,
            8.292051764966633,
            4.4453842092352,
            4.349003461844681,
            6.000704722101477,
            4.543551953871819,
            5.833324443935714,
            3.3688153740004076,
            1.1431546305228522,
            5.498587072359388,
            2.703070385560106,
            3.9334169401183137,
            9.643230396608962,
            3.4603187156827,
            7.691852031027734,
            7.22790045250162,
            4.393578726180066,
            5.243417451059749,
            8.13349302370389,
            5.2583811234088245,
            3.546269300522664,
            3.506822851905734,
            8.301287815488369,
            4.300791041878211,
            6.6587500730057325,
            3.9709462505155106,
            3.362464817847712,
            11.37915331454732,
            8.068138598813995,
            7.916480638467263,
            5.12202392506206,
            8.685405827800933,
            3.6092106401749424,
            5.91911663192843,
            9.953524458486692,
            4.472235413408162,
            5.318791897610933,
            5.403579710683875,
            10.195682092743064,
            14.299048571158615,
            12.38411860673397,
            2.1620234107802583,
            8.521664591626005,
            12.330847080589812,
            3.136419959075777,
            5.542427971288237,
            4.941578224663928,
            4.725295110261525,
            5.165968526411404,
            6.110105031454435,
        ];

        for (actual, expected) in daily_hwd.unwrap().iter().zip(expected_result.iter()) {
            assert_relative_eq!(actual, expected, max_relative = 1e-4)
        }
    }

    // this test does not exist in Python HEM
    #[rstest]
    fn test_remove_pv_diverter_if_present(mut test_input: InputForProcessing) {
        let diverter = json!({
            "StorageTank": "hw cylinder",
            "HeatSource": "immersion"
        });
        let energy_supply_key = ENERGY_SUPPLY_NAME_ELECTRICITY;
        let _ = test_input.add_diverter_to_energy_supply(energy_supply_key, diverter);

        remove_pv_diverter_if_present(&mut test_input).unwrap();
        let energy_supply = test_input
            .energy_supply_by_key(energy_supply_key)
            .unwrap()
            .unwrap();
        assert!(!energy_supply.contains_key("diverter"))
    }

    // this test does not exist in Python HEM
    #[rstest]
    fn test_remove_electric_battery_if_present(mut test_input: InputForProcessing) {
        let electric_battery = json!({
            "capacity": 5,
            "charge_discharge_efficiency_round_trip": 10,
            "battery_age": 2,
            "minimum_charge_rate_one_way_trip": 42,
            "maximum_charge_rate_one_way_trip": 43,
            "maximum_discharge_rate_one_way_trip": 44,
            "battery_location": "inside",
            "grid_charging_possible": false
        });
        let energy_supply_key = ENERGY_SUPPLY_NAME_ELECTRICITY;
        let _ =
            test_input.add_electric_battery_to_energy_supply(energy_supply_key, electric_battery);

        let _ = remove_electric_battery_if_present(&mut test_input);
        let energy_supply = test_input
            .energy_supply_by_key(energy_supply_key)
            .unwrap()
            .unwrap();
        assert!(!energy_supply.contains_key("ElectricBattery"));
    }

    #[rstest]
    fn test_edit_spacecoolsystem_updates_cooling_if_parto_active_cooling_required_is_true(
        mut test_input: InputForProcessing,
    ) {
        test_input.set_part_o_active_cooling_required(true).unwrap();
        let _ = edit_space_cool_system(&mut test_input);
        let space_cool_system = test_input.space_cool_system().unwrap().unwrap();

        for system in space_cool_system.values() {
            assert_eq!(system.get("efficiency").and_then(|v| v.as_f64()), Some(5.1));
            assert_eq!(
                system.get("frac_convective").and_then(|v| v.as_f64()),
                Some(0.95)
            );
            assert_eq!(
                system.get("EnergySupply").and_then(|v| v.as_str()),
                Some(ENERGY_SUPPLY_NAME_ELECTRICITY)
            );
        }
    }

    #[rstest]
    fn test_edit_spacecoolsystem_removes_cooling_if_parto_active_cooling_required_is_false(
        mut test_input: InputForProcessing,
    ) {
        test_input
            .set_part_o_active_cooling_required(false)
            .unwrap();
        let _ = edit_space_cool_system(&mut test_input);
        assert!(!test_input.root().unwrap().contains_key("SpaceCoolSystem"));
        assert!(!test_input.zone_node().unwrap()["whole dwelling"]
            .as_object()
            .unwrap()
            .contains_key("SpaceCoolSystem"));
    }

    // this test does not exist in Python HEM
    #[rstest]
    #[ignore = "This currently fails because test data does not adhere correctly to the FHS schema."]
    fn test_design_capacity(test_input: InputForProcessing) {
        // attempts to coerce the input into something correct
        // test_input.remove_fhs_only_fields().unwrap();
        // create_thermal_penetration(&mut test_input).unwrap();

        let actual_design_capacity = calc_design_capacity(&test_input).unwrap();
        assert_eq!(
            actual_design_capacity.0.get("zone 1").unwrap(),
            &5.356813765662826
        );
        assert_eq!(
            actual_design_capacity.0.get("zone 2").unwrap(),
            &5.356813765662826
        );
        assert_eq!(actual_design_capacity.1, 10.713627531325653);
    }

    // this test does not exist in Python HEM
    #[rstest]
    fn test_initialise_temperature_setpoints(mut test_input: InputForProcessing) {
        initialise_temperature_setpoints(&mut test_input).unwrap();

        let temp_setpoints = test_input.all_init_temp_setpoints().unwrap();

        for temp_setpoint in temp_setpoints {
            assert_eq!(temp_setpoint, Some(18.));
        }
    }

    #[rstest]
    fn test_add_solar_pv_house_only(mut test_input: InputForProcessing, is_fee: bool, tfa: f64) {
        let expected_result = json!({"PV1": {
                "EnergySupply": "mains elec",
                "orientation360": 180.,
                "peak_power": 4.444444444444445,
                "inverter_peak_power_ac": 3.68,
                "inverter_peak_power_dc": 3.68,
                "inverter_is_inside": false,
                "inverter_type": "optimised_inverter",
                "pitch": 45.,
                "type": "PhotovoltaicSystem",
                "ventilation_strategy": "moderately_ventilated",
                "shading": [],
                "base_height": 10.,
                "width": 6.324555320336759,
                "height": 3.1622776601683795
                }
        });

        add_solar_pv(&mut test_input, is_fee, tfa).unwrap();

        let actual_result: IndexMap<String, PhotovoltaicSystem> =
            serde_json::from_value(json!(test_input.on_site_generation().unwrap().unwrap()))
                .unwrap();
        let expected_result: IndexMap<String, PhotovoltaicSystem> =
            serde_json::from_value(expected_result).unwrap();

        assert_eq!(actual_result, expected_result);
    }

    #[rstest]
    fn test_add_solar_pv_house_only_with_multiple_panels(
        mut test_input: InputForProcessing,
        is_fee: bool,
        tfa: f64,
    ) {
        test_input.input["OnSiteGeneration"] = json!({
            "PV1": {
                "EnergySupply": "mains elec",
                "orientation360": 180,
                "peak_power": 5.5,
                "inverter_peak_power_ac": 3.5,
                "inverter_peak_power_dc": 3.5,
                "inverter_type": "optimised_inverter",
                "inverter_is_inside": false,
                "pitch": 45,
                "type": "PhotovoltaicSystem",
                "ventilation_strategy": "moderately_ventilated",
                "shading": [],
                "base_height": 10,
                "width": 1,
                "height": 1,
            },
            "PV2": {
                "EnergySupply": "mains elec",
                "orientation360": 180,
                "peak_power": 7.5,
                "inverter_peak_power_ac": 5.5,
                "inverter_peak_power_dc": 5.5,
                "inverter_type": "optimised_inverter",
                "inverter_is_inside": false,
                "pitch": 45,
                "type": "PhotovoltaicSystem",
                "ventilation_strategy": "moderately_ventilated",
                "shading": [],
                "base_height": 10,
                "width": 1,
                "height": 1,
            },
        });

        let expected_result = json!({
            "PV1": {
                "EnergySupply": "mains elec",
                "orientation360": 180,
                "peak_power": 1.8803418803418803,
                "inverter_peak_power_ac": 3.68,
                "inverter_peak_power_dc": 3.68,
                "inverter_type": "optimised_inverter",
                "inverter_is_inside": false,
                "pitch": 45,
                "type": "PhotovoltaicSystem",
                "ventilation_strategy": "moderately_ventilated",
                "shading": [],
                "base_height": 10,
                "width": 4.1137667560372115,
                "height": 2.0568833780186058,
            },
            "PV2": {
                "EnergySupply": "mains elec",
                "orientation360": 180,
                "peak_power": 2.5641025641025643,
                "inverter_peak_power_ac": 5.5,
                "inverter_peak_power_dc": 5.5,
                "inverter_type": "optimised_inverter",
                "inverter_is_inside": false,
                "pitch": 45,
                "type": "PhotovoltaicSystem",
                "ventilation_strategy": "moderately_ventilated",
                "shading": [],
                "base_height": 10,
                "width": 4.803844614152615,
                "height": 2.4019223070763074,
            },
        });

        add_solar_pv(&mut test_input, is_fee, tfa).unwrap();

        let actual_result: IndexMap<String, PhotovoltaicSystem> =
            serde_json::from_value(json!(test_input.on_site_generation().unwrap().unwrap()))
                .unwrap();
        let expected_result: IndexMap<String, PhotovoltaicSystem> =
            serde_json::from_value(expected_result).unwrap();

        assert_eq!(actual_result, expected_result);
    }

    #[rstest]
    fn test_add_solar_pv_house_only_with_inverter_peak_power_greater_than_min(
        mut test_input: InputForProcessing,
        is_fee: bool,
        tfa: f64,
    ) {
        // Given a house with inverter peak power values in excess of 3.68 kW
        test_input.input["OnSiteGeneration"]["PV1"]["inverter_peak_power_ac"] = json!(4.0);
        test_input.input["OnSiteGeneration"]["PV1"]["inverter_peak_power_dc"] = json!(4.0);
        // When the notional building's PV is determined
        add_solar_pv(&mut test_input, is_fee, tfa).unwrap();
        // Then the inverter peak power values are not modified
        assert_eq!(
            test_input.input["OnSiteGeneration"]["PV1"]["inverter_peak_power_ac"],
            json!(4.0)
        );
        assert_eq!(
            test_input.input["OnSiteGeneration"]["PV1"]["inverter_peak_power_dc"],
            json!(4.0)
        );
    }

    #[rstest]
    fn test_add_solar_pv_house_only_with_inverter_peak_power_less_than_min(
        mut test_input: InputForProcessing,
        is_fee: bool,
        tfa: f64,
    ) {
        // Given a house with inverter peak power values in excess of 3.68 kW
        test_input.input["OnSiteGeneration"]["PV1"]["inverter_peak_power_ac"] = json!(3.0);
        test_input.input["OnSiteGeneration"]["PV1"]["inverter_peak_power_dc"] = json!(3.0);
        // When the notional building's PV is determined
        add_solar_pv(&mut test_input, is_fee, tfa).unwrap();
        // Then the inverter peak power values are not modified
        assert_eq!(
            test_input.input["OnSiteGeneration"]["PV1"]["inverter_peak_power_ac"],
            json!(3.68)
        );
        assert_eq!(
            test_input.input["OnSiteGeneration"]["PV1"]["inverter_peak_power_dc"],
            json!(3.68)
        );
    }

    #[rstest]
    fn test_add_solar_pv_does_not_modify_flats_in_tall_buildings(
        mut test_input: InputForProcessing,
        is_fee: bool,
        tfa: f64,
    ) {
        let previous_on_site_generation = json!(test_input.on_site_generation().unwrap().unwrap());
        test_input.input["General"] = json!({
            "build_type": "flat",
            "storey_of_dwelling": 3,
            "storeys_in_building": 16,
            "storeys_in_dwelling": 1,
        });
        add_solar_pv(&mut test_input, is_fee, tfa).unwrap();
        assert_eq!(
            previous_on_site_generation,
            json!(test_input.on_site_generation().unwrap().unwrap())
        );
    }

    #[rstest]
    fn test_add_solar_pv_does_modify_flats_in_short_buildings(
        mut test_input: InputForProcessing,
        is_fee: bool,
        tfa: f64,
    ) {
        let previous_on_site_generation = json!(test_input.on_site_generation().unwrap().unwrap());

        // Given a building with 15 storeys
        test_input.input["General"] = json!({
            "build_type": "flat",
            "storey_of_dwelling": 3,
            "storeys_in_building": 15,
            "storeys_in_dwelling": 1,
        });
        // When the notional building's PV is determined
        add_solar_pv(&mut test_input, is_fee, tfa).unwrap();
        // Then it differs from the actual
        let actual_on_site_generation = json!(test_input.on_site_generation().unwrap().unwrap());
        assert_ne!(previous_on_site_generation, actual_on_site_generation);

        let expected_on_site_generation = json!({
            "PV1": {
                "EnergySupply": "mains elec",
                "orientation360": 180,
                "peak_power": 0.9481481481481482,
                "inverter_peak_power_ac": 3.68,
                "inverter_peak_power_dc": 3.68,
                "inverter_type": "optimised_inverter",
                "inverter_is_inside": false,
                "pitch": 45,
                "type": "PhotovoltaicSystem",
                "ventilation_strategy": "moderately_ventilated",
                "shading": [],
                "base_height": 10,
                "width": 2.9211869733608857,
                "height": 1.4605934866804429,
            }
        });
        assert_eq!(expected_on_site_generation, actual_on_site_generation);
    }

    #[fixture]
    fn cold_water_source(test_input: InputForProcessing) -> std::string::String {
        test_input.input["ColdWaterSource"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .to_string()
    }

    #[rstest]
    #[ignore = "This currently fails because test data does not adhere correctly to the FHS schema."]
    fn test_non_sleeved_district_heat_network(
        mut test_input: InputForProcessing,
        cold_water_source: std::string::String,
        is_fee: bool,
        tfa: f64,
    ) {
        // Given an unsleeved DHN network
        // When the notional heating system is created
        edit_space_heating_system(
            &mut test_input,
            &cold_water_source,
            tfa,
            HeatNetworkType::UnsleevedDhn.into(),
            &Default::default(),
            is_fee,
        )
        .unwrap();

        // Then a notional heat pump is created, not a heat network
        let expected_heat_source_wet: IndexMap<String, HeatSourceWetDetails> =
            serde_json::from_value(json!(
             {
                "notional_HP": {
                    "EnergySupply": "mains elec",
                    "backup_ctrl_type": "TopUp",
                    "min_modulation_rate_35": 0.4,
                    "min_modulation_rate_55": 0.4,
                    "min_temp_diff_flow_return_for_hp_to_operate": 0,
                    "modulating_control": true,
                    "power_crankcase_heater": 0.01,
                    "power_heating_circ_pump": 1.03 * 0.003,
                    "power_max_backup": 3,
                    "power_off": 0,
                    "power_source_circ_pump": 0.01,
                    "power_standby": 0.01,
                    "sink_type": "Water",
                    "source_type": "OutsideAir",
                    "temp_lower_operating_limit": -10,
                    "temp_return_feed_max": 60,
                    "test_data_EN14825": [
                        {
                            "capacity": 1.00,
                            "cop": 2.79,
                            "design_flow_temp": 35,
                            "temp_outlet": 34,
                            "temp_source": -7,
                            "temp_test": -7,
                            "test_letter": "A",
                        },
                        {
                            "capacity": 0.62,
                            "cop": 4.29,
                            "design_flow_temp": 35,
                            "temp_outlet": 30,
                            "temp_source": 2,
                            "temp_test": 2,
                            "test_letter": "B",
                        },
                        {
                            "capacity": 0.55,
                            "cop": 5.91,
                            "design_flow_temp": 35,
                            "temp_outlet": 27,
                            "temp_source": 7,
                            "temp_test": 7,
                            "test_letter": "C",
                        },
                        {
                            "capacity": 0.47,
                            "cop": 8.02,
                            "design_flow_temp": 35,
                            "temp_outlet": 24,
                            "temp_source": 12,
                            "temp_test": 12,
                            "test_letter": "D",
                        },
                        {
                            "capacity": 1.05,
                            "cop": 2.49,
                            "design_flow_temp": 35,
                            "temp_outlet": 35,
                            "temp_source": -10,
                            "temp_test": -10,
                            "test_letter": "F",
                        },
                        {
                            "capacity": 0.99,
                            "cop": 2.03,
                            "design_flow_temp": 55,
                            "temp_outlet": 52,
                            "temp_source": -7,
                            "temp_test": -7,
                            "test_letter": "A",
                        },
                        {
                            "capacity": 0.60,
                            "cop": 3.12,
                            "design_flow_temp": 55,
                            "temp_outlet": 42,
                            "temp_source": 2,
                            "temp_test": 2,
                            "test_letter": "B",
                        },
                        {
                            "capacity": 0.49,
                            "cop": 4.41,
                            "design_flow_temp": 55,
                            "temp_outlet": 36,
                            "temp_source": 7,
                            "temp_test": 7,
                            "test_letter": "C",
                        },
                        {
                            "capacity": 0.51,
                            "cop": 6.30,
                            "design_flow_temp": 55,
                            "temp_outlet": 30,
                            "temp_source": 12,
                            "temp_test": 12,
                            "test_letter": "D",
                        },
                        {
                            "capacity": 1.03,
                            "cop": 1.87,
                            "design_flow_temp": 55,
                            "temp_outlet": 55,
                            "temp_source": -10,
                            "temp_test": -10,
                            "test_letter": "F",
                        },
                    ],
                    "time_constant_onoff_operation": 120,
                    "time_delay_backup": 1,
                    "type": "HeatPump",
                    "var_flow_temp_ctrl_during_test": true,
                }
            }))
            .unwrap();
        let actual_heat_source_wet: IndexMap<String, HeatSourceWetDetails> =
            serde_json::from_value(test_input.input["HeatSourceWet"].clone()).unwrap();
        assert_eq!(actual_heat_source_wet, expected_heat_source_wet);

        let expected_hot_water_source: IndexMap<String, HotWaterSourceDetails> =
            serde_json::from_value(json!({
                "hw cylinder": {
                    "ColdWaterSource": &cold_water_source,
                    "HeatSource": {
                        "notional_HP": {
                            "heater_position": 0.1,
                            "name": "notional_HP",
                            "temp_flow_limit_upper": 60,
                            "thermostat_position": 0.1,
                            "type": "HeatSourceWet",
                        }
                    },
                    "daily_losses": 0.46660029577109363,
                    "type": "StorageTank",
                    "volume": 80.0,
                    "primary_pipework": [
                        {
                            "external_diameter_mm": 28,
                            "insulation_thermal_conductivity": 0.035,
                            "insulation_thickness_mm": 35,
                            "internal_diameter_mm": 26,
                            "length": 2.5,
                            "location": "internal",
                            "pipe_contents": "water",
                            "surface_reflectivity": false,
                        }
                    ],
                }
            }))
            .unwrap();
        let actual_hot_water_source: IndexMap<String, HotWaterSourceDetails> =
            serde_json::from_value(test_input.input["HotWaterSource"].clone()).unwrap();
        assert_eq!(actual_hot_water_source, expected_hot_water_source);

        // And the SpaceHeatSystem is replaced with a notional one
        assert_eq!(
            test_input.input["Zone"]["whole dwelling"]["SpaceHeatSystem"]
                .as_str()
                .unwrap(),
            "whole dwelling_SpaceHeatSystem_Notional"
        );

        let expected_space_heat_system: IndexMap<String, SpaceHeatSystemDetails> =
            serde_json::from_value(json!({
                "whole dwelling_SpaceHeatSystem_Notional": {
                    "type": "WetDistribution",
                    "thermal_mass": 0.028777777777777777,
                    "emitters": [
                        {
                            "c": 0.0199927250791513,
                            "frac_convective": 0.7,
                            "n": 1.34,
                            "wet_emitter_type": "radiator",
                        }
                    ],
                    "temp_diff_emit_dsgn": 5,
                    "variable_flow": false,
                    "HeatSource": {"name": "notional_HP", "temp_flow_limit_upper": 65.0},
                    "ecodesign_controller": {
                        "ecodesign_control_class": 2,
                        "max_outdoor_temp": 20,
                        "min_flow_temp": 21,
                        "min_outdoor_temp": 0,
                    },
                    "Control": "HeatingPattern_Null",
                    "design_flow_temp": 45,
                    "design_flow_rate": 12,
                    "Zone": "whole dwelling",
                    "pipework": [],
                }
            }))
            .unwrap();
        let actual_space_heat_system: IndexMap<String, SpaceHeatSystemDetails> =
            serde_json::from_value(test_input.input["SpaceHeatSystem"].clone()).unwrap();
        assert_eq!(actual_space_heat_system, expected_space_heat_system);
    }

    #[rstest]
    #[ignore = "This currently fails because test data does not adhere correctly to the FHS schema."]
    fn test_sleeved_dhn_heat_network(
        mut test_input: InputForProcessing,
        cold_water_source: std::string::String,
        is_fee: bool,
        tfa: f64,
    ) {
        let custom_energy_factors: IndexMap<Arc<str>, CustomEnergySourceFactor> =
            serde_json::from_value(json!({
                "custom_1": {
                    "Emissions Factor kgCO2e/kWh": 1,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 1,
                    "Primary Energy Factor kWh/kWh delivered": 1,
                },
                "custom_2": {
                    "Emissions Factor kgCO2e/kWh": 0.5,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 0.5,
                    "Primary Energy Factor kWh/kWh delivered": 0.5,
                },
            }))
            .unwrap();

        // When the notional heating system is created
        let new_factors = edit_space_heating_system(
            &mut test_input,
            &cold_water_source,
            tfa,
            HeatNetworkType::SleevedDhn.into(),
            &custom_energy_factors,
            is_fee,
        )
        .unwrap();
        // Then the heating system created is a heat network with an HIU

        let expected_heat_source_wet: IndexMap<String, HeatSourceWetDetails> =
            serde_json::from_value(json!({
                "notionalHIU": {
                    "type": "HIU",
                    "EnergySupply": "_notional_heat_network",
                    "power_max": 45,
                    "HIU_daily_loss": 0.8,
                    "building_level_distribution_losses": 62,
                }
            }))
            .unwrap();
        let actual_heat_source_wet: IndexMap<String, HeatSourceWetDetails> =
            serde_json::from_value(test_input.input["HeatSourceWet"].clone()).unwrap();
        assert_eq!(actual_heat_source_wet, expected_heat_source_wet);

        let expected_hot_water_source: IndexMap<String, HotWaterSourceDetails> =
            serde_json::from_value(json!({
                "hw cylinder": {
                    "type": "HIU",
                    "ColdWaterSource": &cold_water_source,
                    "HeatSourceWet": "notionalHIU",
                }
            }))
            .unwrap();
        let actual_hot_water_source: IndexMap<String, HotWaterSourceDetails> =
            serde_json::from_value(test_input.input["HotWaterSource"].clone()).unwrap();
        assert_eq!(actual_hot_water_source, expected_hot_water_source);

        // And the SpaceHeatSystem is replaced with a notional one
        assert_eq!(
            test_input.input["Zone"]["whole dwelling"]["SpaceHeatSystem"]
                .as_str()
                .unwrap(),
            "whole dwelling_SpaceHeatSystem_Notional"
        );
        let expected_space_heat_system: IndexMap<String, SpaceHeatSystemDetails> =
            serde_json::from_value(json!({
                "whole dwelling_SpaceHeatSystem_Notional": {
                    "type": "WetDistribution",
                    "thermal_mass": 0.014388888888888889,
                    "emitters": [
                        {
                            "c": 0.00999636253957565,
                            "frac_convective": 0.7,
                            "n": 1.34,
                            "wet_emitter_type": "radiator",
                        }
                    ],
                    "temp_diff_emit_dsgn": 20,
                    "variable_flow": false,
                    "HeatSource": {"name": "notionalHIU", "temp_flow_limit_upper": 65.0},
                    "ecodesign_controller": {"ecodesign_control_class": 1},
                    "Control": "HeatingPattern_Null",
                    "design_flow_temp": 55,
                    "design_flow_rate": 8,
                    "Zone": "whole dwelling",
                    "pipework": [],
                }
            }))
            .unwrap();
        let actual_space_heat_system: IndexMap<String, SpaceHeatSystemDetails> =
            serde_json::from_value(test_input.input["SpaceHeatSystem"].clone()).unwrap();
        assert_eq!(actual_space_heat_system, expected_space_heat_system);

        // And the custom energy factors have the notional heat network has
        // the mean factors of the actual custom supplies
        let expected_custom_energy_factors: IndexMap<Arc<str>, CustomEnergySourceFactor> =
            serde_json::from_value(json!({
                "_notional_heat_network": {
                    "Emissions Factor kgCO2e/kWh": 0.75,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 0.75,
                    "Primary Energy Factor kWh/kWh delivered": 0.75,
                }
            }))
            .unwrap();
        assert_eq!(new_factors, expected_custom_energy_factors);

        // And the notional custom EnergySupply is added
        assert_eq!(
            test_input.input["EnergySupply"]["_notional_heat_network"].clone(),
            json!({"fuel": "custom", "is_export_capable": false})
        );

        // And the original custom EnergySupply is removed
        assert!(!test_input.input["EnergySupply"]
            .as_object()
            .unwrap()
            .contains_key("custom_1"));
    }

    #[rstest]
    #[ignore = "This currently fails because test data does not adhere correctly to the FHS schema."]
    fn test_communal_heat_network(
        mut test_input: InputForProcessing,
        cold_water_source: std::string::String,
        is_fee: bool,
        tfa: f64,
    ) {
        let custom_energy_factors: IndexMap<Arc<str>, CustomEnergySourceFactor> =
            serde_json::from_value(json!({
                "custom_1": {
                    "Emissions Factor kgCO2e/kWh": 1,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 1,
                    "Primary Energy Factor kWh/kWh delivered": 1,
                }
            }))
            .unwrap();
        test_input.input["HeatSourceWet"] = json!( {
            "HeatNetwork": {
                "type": "HIU",
                "EnergySupply": "custom_1",
                "power_max": 3.0,
                "HIU_daily_loss": 0.8,
                "building_level_distribution_losses": 62,
                "is_heat_network": true,
                "heat_network_type": "communal",
            }
        });
        test_input.input["EnergySupply"]["custom_1"] =
            json!({"is_export_capable": false, "fuel": "custom"});

        // When the notional heating system is created
        let heat_network_type =
            test_input.input["HeatSourceWet"]["HeatNetwork"]["heat_network_type"].clone();
        let new_factors = edit_space_heating_system(
            &mut test_input,
            &cold_water_source,
            tfa,
            serde_json::from_value(heat_network_type).unwrap(),
            &custom_energy_factors,
            is_fee,
        )
        .unwrap();

        // Then the heating system created is a heat network with a HIU

        let expected_heat_source_wet: IndexMap<String, HeatSourceWetDetails> =
            serde_json::from_value(json!({
                "notionalHIU": {
                    "type": "HIU",
                    "EnergySupply": "_notional_heat_network",
                    "power_max": 45,
                    "HIU_daily_loss": 0.8,
                    "building_level_distribution_losses": 62,
                }
            }))
            .unwrap();
        let actual_heat_source_wet: IndexMap<String, HeatSourceWetDetails> =
            serde_json::from_value(test_input.input["HeatSourceWet"].clone()).unwrap();
        assert_eq!(actual_heat_source_wet, expected_heat_source_wet);

        let expected_hot_water_source: IndexMap<String, HotWaterSourceDetails> =
            serde_json::from_value(json!({
                "hw cylinder": {
                    "type": "HIU",
                    "ColdWaterSource": &cold_water_source,
                    "HeatSourceWet": "notionalHIU",
                }
            }))
            .unwrap();
        let actual_hot_water_source: IndexMap<String, HotWaterSourceDetails> =
            serde_json::from_value(test_input.input["HotWaterSource"].clone()).unwrap();
        assert_eq!(actual_hot_water_source, expected_hot_water_source);

        // And the SpaceHeatSystem is replaced with a notional one
        assert_eq!(
            test_input.input["Zone"]["whole dwelling"]["SpaceHeatSystem"]
                .as_str()
                .unwrap(),
            "whole dwelling_SpaceHeatSystem_Notional"
        );
        let expected_space_heat_system: IndexMap<String, SpaceHeatSystemDetails> =
            serde_json::from_value(json!({
                "whole dwelling_SpaceHeatSystem_Notional": {
                    "type": "WetDistribution",
                    "thermal_mass": 0.014388888888888889,
                    "emitters": [
                        {
                            "c": 0.00999636253957565,
                            "frac_convective": 0.7,
                            "n": 1.34,
                            "wet_emitter_type": "radiator",
                        }
                    ],
                    "temp_diff_emit_dsgn": 20,
                    "variable_flow": false,
                    "HeatSource": {"name": "notionalHIU", "temp_flow_limit_upper": 65.0},
                    "ecodesign_controller": {"ecodesign_control_class": 1},
                    "Control": "HeatingPattern_Null",
                    "design_flow_temp": 55,
                    "design_flow_rate": 8,
                    "Zone": "whole dwelling",
                    "pipework": [],
                }
            }))
            .unwrap();
        let actual_space_heat_system: IndexMap<String, SpaceHeatSystemDetails> =
            serde_json::from_value(test_input.input["SpaceHeatSystem"].clone()).unwrap();
        assert_eq!(actual_space_heat_system, expected_space_heat_system);

        // And the custom energy factors have the standardised factors of a communal system
        let expected_custom_energy_factors: IndexMap<Arc<str>, CustomEnergySourceFactor> =
            serde_json::from_value(json!({
                "_notional_heat_network": {
                    "Emissions Factor kgCO2e/kWh": 0.033,
                    "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 0.033,
                    "Primary Energy Factor kWh/kWh delivered": 0.75,
                }
            }))
            .unwrap();
        assert_eq!(new_factors, expected_custom_energy_factors);

        // And the notional custom EnergySupply is added
        assert_eq!(
            test_input.input["EnergySupply"]["_notional_heat_network"].clone(),
            json!({"fuel": "custom", "is_export_capable": false})
        );

        // And the original custom EnergySupply is removed
        assert!(!test_input.input["EnergySupply"]
            .as_object()
            .unwrap()
            .contains_key("custom_1"));
    }

    #[rstest]
    fn test_actual_with_centralised_mechanical_ventilation_system(
        mut test_input: InputForProcessing,
    ) {
        test_input.input["InfiltrationVentilation"]["MechanicalVentilation"] = json!({
            "mechvent1": {
                "vent_type": "Centralised continuous MEV",
                "measured_fan_power": 12.26,
                "measured_air_flow_rate": 37.,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 80.,
                "mid_height_air_flow_path": 1.5,
                "orientation360": 90.,
                "pitch": 60.,
            }
        });
        let minimum_air_flow_rate = 100.;
        let minimum_vent_area = 200.;
        let minimum_vent_count = 4;

        // When the notional infiltration ventilation preprocessing is applied
        edit_infiltration_ventilation(
            &mut test_input,
            minimum_air_flow_rate,
            minimum_vent_area,
            minimum_vent_count,
        )
        .unwrap();

        // Then two dMEVs are created in the notional with numbered vent names
        let expected_mech_vent = json!({
            "Decentralised_Continuous_MEV_0": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 50.0,
                "mid_height_air_flow_path": 2.25,
                "orientation360": 90.,
                "pitch": 90.,
            },
            "Decentralised_Continuous_MEV_1": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 50.0,
                "mid_height_air_flow_path": 1.25,
                "orientation360": 0.,
                "pitch": 90.,
            },
        });
        let actual_mech_vent =
            test_input.input["InfiltrationVentilation"]["MechanicalVentilation"].clone();
        assert_eq!(actual_mech_vent, expected_mech_vent);
    }
}
