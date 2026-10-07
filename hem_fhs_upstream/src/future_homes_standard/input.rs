use crate::future_homes_standard::fhs_schema_validation::apply_schema_validation;
use anyhow::anyhow;
use home_energy_model::hem_core::simulation_time::SimulationTime;
use home_energy_model::input::{
    ColdWaterSourceInput, ExternalConditionsInput, Input, WasteWaterHeatRecovery,
    WaterDistribution, WaterHeatingEvent, WaterPipework,
};
use home_energy_model::input::{
    Control, EnergySupplyInput, InfiltrationVentilation, InputForCalcHtcHlp, ZoneDictionary,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as JsonValue};
use serde_valid::Validate;
use std::collections::HashSet;
use std::io::{BufReader, Read};
use std::sync::Arc;
use thiserror::Error;

pub(crate) fn ingest_for_processing(json: impl Read) -> Result<InputForProcessing, anyhow::Error> {
    InputForProcessing::init_with_json(json)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InputForProcessing {
    pub(crate) input: JsonValue,
}

impl InputForProcessing {
    pub fn init_with_json(json: impl Read) -> Result<Self, anyhow::Error> {
        let input_for_processing = Self::init_with_json_skip_validation(json)?;

        apply_schema_validation(&input_for_processing.input)?;

        Ok(input_for_processing)
    }

    pub(crate) fn init_with_json_skip_validation(json: impl Read) -> Result<Self, anyhow::Error> {
        let reader = BufReader::new(json);

        let input: JsonValue = serde_json::from_reader(reader)?;

        Ok(Self { input })
    }

    pub(crate) fn as_input_for_calc_htc_hlp(&self) -> anyhow::Result<ReducedInputForCalcHtcHlp> {
        let mut input_to_reduce = InputForProcessing {
            input: self.input.clone(),
        };

        // remove FHS specific fields
        input_to_reduce.remove_fhs_only_fields()?;

        serde_json::from_value(input_to_reduce.input).map_err(|err| anyhow!(err))
    }

    pub fn finalize(self) -> anyhow::Result<Input> {
        // NB. this _might_ in time be a good point to perform a validation against the core schema - or it might not
        // if let BasicOutput::Invalid(errors) =
        //     CORE_INCLUDING_FHS_VALIDATOR.apply(&self.input).basic()
        // {
        //     bail!(
        //         "Wrapper formed invalid JSON for the core schema: {}",
        //         serde_json::to_value(errors)?.to_json_string_pretty()?
        //     );
        // }
        serde_json::from_value(self.input).map_err(|err| anyhow!(err))
    }

    pub fn root(&self) -> JsonAccessResult<&Map<std::string::String, JsonValue>> {
        self.input
            .as_object()
            .ok_or(json_error("Root document is not an object"))
    }

    fn root_mut(&mut self) -> JsonAccessResult<&mut Map<std::string::String, JsonValue>> {
        self.input
            .as_object_mut()
            .ok_or(json_error("Root document is not an object"))
    }

    fn set_on_root_key(&mut self, root_key: &str, value: JsonValue) -> JsonAccessResult<&mut Self> {
        self.root_mut()?.insert(root_key.into(), value);

        Ok(self)
    }

    fn remove_root_key(&mut self, root_key: &str) -> JsonAccessResult<&mut Self> {
        self.root_mut()?.shift_remove(root_key);

        Ok(self)
    }

    fn root_object(
        &self,
        root_key: &str,
    ) -> JsonAccessResult<&Map<std::string::String, JsonValue>> {
        self.root()?
            .get(root_key)
            .ok_or(json_error(format!("No {root_key} node found")))?
            .as_object()
            .ok_or(json_error(format!("{root_key} node was not an object")))
    }

    fn root_object_mut(
        &mut self,
        root_key: &str,
    ) -> JsonAccessResult<&mut Map<std::string::String, JsonValue>> {
        self.root_mut()?
            .get_mut(root_key)
            .ok_or(json_error(format!("No {root_key} node found")))?
            .as_object_mut()
            .ok_or(json_error(format!("{root_key} node was not an object")))
    }

    /// Uses entry API to ensure that root key is created if it does not already exist.
    pub(crate) fn root_object_entry_mut(
        &mut self,
        root_key: &str,
    ) -> JsonAccessResult<&mut Map<std::string::String, JsonValue>> {
        self.root_mut()?
            .entry(root_key)
            .or_insert(json!({}))
            .as_object_mut()
            .ok_or(json_error(format!("{root_key} node was not an object")))
    }

    fn optional_root_object(
        &self,
        root_key: &str,
    ) -> JsonAccessResult<Option<&Map<std::string::String, JsonValue>>> {
        Ok(self.root()?.get(root_key).and_then(|v| v.as_object()))
    }

    pub fn optional_root_object_mut(
        &mut self,
        root_key: &str,
    ) -> JsonAccessResult<Option<&mut Map<std::string::String, JsonValue>>> {
        Ok(self
            .root_mut()?
            .get_mut(root_key)
            .and_then(|v| v.as_object_mut()))
    }

    pub fn set_simulation_time(
        &mut self,
        simulation_time: SimulationTime,
    ) -> anyhow::Result<&mut Self> {
        self.set_on_root_key("SimulationTime", serde_json::to_value(simulation_time)?)
            .map_err(Into::into)
    }

    pub fn set_temp_internal_air_static_calcs(
        &mut self,
        temp_internal_air_static_calcs: Option<f64>,
    ) -> JsonAccessResult<&mut Self> {
        self.set_on_root_key(
            "temp_internal_air_static_calcs",
            temp_internal_air_static_calcs.into(),
        )
    }

    pub(crate) fn merge_external_conditions_data(
        &mut self,
        external_conditions: ExternalConditionsInput,
    ) -> anyhow::Result<()> {
        let shading_segments = self
            .root_object("ExternalConditions")?
            .get("shading_segments")
            .cloned()
            .unwrap_or(json!([]));
        let mut new_external_conditions = serde_json::to_value(external_conditions)?;
        let new_external_conditions_map = new_external_conditions
            .as_object_mut()
            .ok_or(json_error(
            "External conditions was not a JSON object when it was expected to be provided as one",
        ))?;
        new_external_conditions_map.insert("shading_segments".into(), shading_segments);
        self.set_on_root_key("ExternalConditions", new_external_conditions)?;

        Ok(())
    }

    pub fn reset_internal_gains(&mut self) -> JsonAccessResult<&Self> {
        self.root_mut()?.insert("InternalGains".into(), json!({}));

        Ok(self)
    }

    pub fn reset_control(&mut self) -> JsonAccessResult<&Self> {
        self.root_mut()?.insert("Control".into(), json!({}));
        Ok(self)
    }

    pub(crate) fn zone_node(&self) -> JsonAccessResult<&Map<String, JsonValue>> {
        self.root_object("Zone")
    }

    pub fn zone_node_mut(
        &mut self,
    ) -> JsonAccessResult<&mut serde_json::Map<std::string::String, JsonValue>> {
        self.root_object_mut("Zone")
    }

    pub fn specific_zone(
        &self,
        zone_key: &str,
    ) -> JsonAccessResult<&serde_json::Map<std::string::String, JsonValue>> {
        self.zone_node()?
            .get(zone_key)
            .ok_or(json_error(format!("Zone key {zone_key} did not exist")))?
            .as_object()
            .ok_or(json_error("Zone node was not an object"))
    }

    fn specific_zone_mut(
        &mut self,
        zone_key: &str,
    ) -> JsonAccessResult<&mut serde_json::Map<std::string::String, JsonValue>> {
        self.zone_node_mut()?
            .get_mut(zone_key)
            .ok_or(json_error(format!("Zone key {zone_key} did not exist")))?
            .as_object_mut()
            .ok_or(json_error("Zone node was not an object"))
    }

    pub fn total_zone_area(&self) -> JsonAccessResult<f64> {
        self.zone_node()?
            .values()
            .map(|z| {
                z.get("area")
                    .ok_or(json_error("Area field not found on zone"))?
                    .as_f64()
                    .ok_or(json_error("Area field not a number"))
            })
            .sum::<JsonAccessResult<f64>>()
    }

    pub fn area_for_zone(&self, zone: &str) -> anyhow::Result<f64> {
        Ok(self
            .zone_node()?
            .get(zone)
            .ok_or(anyhow!("Used zone key for a zone that does not exist"))?
            .get("area")
            .ok_or(json_error("Area not found on zone"))?
            .as_number()
            .ok_or(json_error("Area on zone was not a number"))?
            .as_f64()
            .ok_or(json_error("Area number could not be read as a number"))?)
    }

    pub(crate) fn living_room_area_for_zone(&self, zone: &str) -> anyhow::Result<f64> {
        Ok(self
            .zone_node()?
            .get(zone)
            .ok_or(anyhow!("Used zone key for a zone that does not exist"))?
            .get("livingroom_area")
            .ok_or(json_error("Living room area not found on zone"))?
            .as_f64()
            .ok_or(json_error(
                "Living room area number could not be read as a number",
            ))?)
    }

    pub(crate) fn rest_of_dwelling_area_for_zone(&self, zone: &str) -> anyhow::Result<f64> {
        Ok(self
            .zone_node()?
            .get(zone)
            .ok_or(anyhow!("Used zone key for a zone that does not exist"))?
            .get("restofdwelling_area")
            .ok_or(json_error("Rest of dwelling area not found on zone"))?
            .as_f64()
            .ok_or(json_error(
                "Rest of dwelling area number could not be read as a number",
            ))?)
    }

    #[cfg(test)]
    pub(crate) fn all_thermal_bridgings(&self) -> JsonAccessResult<Vec<&JsonValue>> {
        Ok(self
            .zone_node()?
            .values()
            .flat_map(|z| z.get("ThermalBridging"))
            .collect::<Vec<_>>())
    }

    pub(crate) fn all_thermal_bridging_elements(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut Map<std::string::String, JsonValue>>> {
        let zones = self.zone_node_mut()?;
        let mut result = Vec::new();
        for zone in zones.values_mut() {
            if let Some(thermal_bridging) = zone.get_mut("ThermalBridging") {
                if let Some(obj) = thermal_bridging.as_object_mut() {
                    result.push(obj);
                }
            }
        }

        Ok(result)
    }

    pub fn number_of_bedrooms(&self) -> JsonAccessResult<usize> {
        Ok(self
            .input
            .get("NumberOfBedrooms")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| json_error("NumberOfBedrooms not available as non-negative integer"))?
            as usize)
    }

    pub fn number_of_habitable_rooms(&self) -> JsonAccessResult<usize> {
        Ok(self
            .input
            .get("NumberOfHabitableRooms")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| json_error("NumberOfHabitableRooms not available as positive integer"))?
            as usize)
    }

    pub(crate) fn number_of_wet_rooms(&self) -> JsonAccessResult<Option<usize>> {
        match self.input.get("NumberOfWetRooms") {
            None => Ok(None),
            Some(JsonValue::Number(n)) => Ok(Some(
                n.as_u64()
                    .ok_or(json_error("NumberOfWetRooms not a positive integer"))?
                    as usize,
            )),
            Some(_) => Err(json_error("NumberOfWetRooms not a number")),
        }
    }

    pub(crate) fn number_of_bathrooms(&self) -> JsonAccessResult<usize> {
        Ok(self
            .input
            .get("NumberOfBathrooms")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| json_error("NumberOfBathrooms not available as positive integer"))?
            as usize)
    }

    pub(crate) fn number_of_utility_rooms(&self) -> JsonAccessResult<usize> {
        Ok(self
            .input
            .get("NumberOfUtilityRooms")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| json_error("NumberOfUtilityRooms not available as positive integer"))?
            as usize)
    }

    pub(crate) fn number_of_sanitary_accommodations(&self) -> JsonAccessResult<usize> {
        Ok(self
            .input
            .get("NumberOfSanitaryAccommodations")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| {
                json_error("NumberOfSanitaryAccommodations not available as positive integer")
            })? as usize)
    }

    pub(super) fn number_of_hot_tapped_rooms(&self) -> JsonAccessResult<usize> {
        match self.input.get("NumberOfHotTappedRooms") {
            Some(JsonValue::Number(n)) => Ok(n
                .as_u64()
                .ok_or(json_error("NumberOfHotTappedRooms not a positive integer"))?
                as usize),
            _ => Err(json_error(
                "NumberOfHotTappedRooms not found or not a number",
            )),
        }
    }

    pub(crate) fn kitchen_extractor_hood_external(&self) -> JsonAccessResult<bool> {
        self.input
            .get("KitchenExtractorHoodExternal")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| json_error("Extractor hood external not found or not a boolean"))
    }

    fn internal_gains_mut(&mut self) -> JsonAccessResult<&mut Map<std::string::String, JsonValue>> {
        self.root_object_entry_mut("InternalGains")
    }

    pub fn set_metabolic_gains(
        &mut self,
        start_day: u32,
        time_series_step: f64,
        schedule_json: JsonValue,
    ) -> anyhow::Result<&Self> {
        self.internal_gains_mut()?.insert(
            "metabolic gains".into(),
            json!({
                "start_day": start_day,
                "time_series_step": time_series_step,
                "schedule": schedule_json,
            }),
        );

        Ok(self)
    }

    pub fn set_evaporative_losses(
        &mut self,
        start_day: u32,
        time_series_step: f64,
        schedule_json: JsonValue,
    ) -> anyhow::Result<&Self> {
        self.internal_gains_mut()?.insert(
            "EvaporativeLosses".into(),
            json!({
                "start_day": start_day,
                "time_series_step": time_series_step,
                "schedule": schedule_json,
            }),
        );

        Ok(self)
    }

    pub fn set_cold_water_losses(
        &mut self,
        start_day: u32,
        time_series_step: f64,
        schedule_json: JsonValue,
    ) -> anyhow::Result<&Self> {
        self.internal_gains_mut()?.insert(
            "ColdWaterLosses".into(),
            json!({
                "start_day": start_day,
                "time_series_step": time_series_step,
                "schedule": schedule_json,
            }),
        );

        Ok(self)
    }

    pub fn set_heating_control_type(
        &mut self,
        heating_control_type_value: JsonValue,
    ) -> anyhow::Result<&mut Self> {
        self.set_on_root_key("HeatingControlType", heating_control_type_value)
            .map_err(Into::into)
    }

    pub fn add_control(
        &mut self,
        control_key: &str,
        control_json: JsonValue,
    ) -> JsonAccessResult<&Self> {
        self.root_object_entry_mut("Control")?
            .insert(control_key.into(), control_json);

        Ok(self)
    }

    pub(crate) fn remove_preheated_water_sources(&mut self) -> JsonAccessResult<&mut Self> {
        self.remove_root_key("PreHeatedWaterSource")
    }

    pub(crate) fn has_preheated_water_source(&self) -> JsonAccessResult<bool> {
        Ok(self.root()?.contains_key("PreHeatedWaterSource"))
    }

    pub(crate) fn all_preheated_tank_heat_source_values_mut(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut JsonValue>> {
        Ok(self
            .root_object_mut("PreHeatedWaterSource")?
            .get_mut("preheated tank")
            .ok_or_else(|| json_error("preheated tank not found"))?
            .get_mut("HeatSource")
            .ok_or_else(|| json_error("HeatSource not found"))?
            .as_object_mut()
            .ok_or_else(|| json_error("HeatSource not an object"))?
            .values_mut()
            .collect())
    }

    pub fn zone_keys(&self) -> JsonAccessResult<Vec<smartstring::alias::String>> {
        Ok(self
            .zone_node()?
            .keys()
            .map(smartstring::alias::String::from)
            .collect())
    }

    #[cfg(test)]
    pub(crate) fn all_init_temp_setpoints(&self) -> JsonAccessResult<Vec<Option<f64>>> {
        Ok(self
            .zone_node()?
            .values()
            .map(|zone| zone.get("temp_setpnt_init").and_then(|t| t.as_f64()))
            .collect())
    }

    pub fn set_init_temp_setpoint_for_zone(
        &mut self,
        zone: &str,
        temperature: f64,
    ) -> JsonAccessResult<&Self> {
        self.specific_zone_mut(zone)?
            .insert("temp_setpnt_init".into(), json!(temperature));
        Ok(self)
    }

    pub(crate) fn set_area_for_zone(&mut self, zone: &str, area: f64) -> JsonAccessResult<&Self> {
        self.specific_zone_mut(zone)?
            .insert("area".into(), json!(area));
        Ok(self)
    }

    pub fn space_heat_system_for_zone(
        &self,
        zone: &str,
    ) -> JsonAccessResult<Vec<smartstring::alias::String>> {
        Ok(match self.specific_zone(zone)?.get("SpaceHeatSystem") {
            Some(JsonValue::String(system)) => vec![smartstring::alias::String::from(system)],
            Some(JsonValue::Array(systems)) => systems
                .iter()
                .map(|system| {
                    Ok(smartstring::alias::String::from(system.as_str().ok_or(
                        json_error("Space heat system list contained a non-string"),
                    )?))
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => vec![],
        })
    }

    pub fn set_space_heat_system_for_zone(
        &mut self,
        zone: &str,
        system_name: &str,
    ) -> anyhow::Result<&Self> {
        let zone = self.specific_zone_mut(zone)?;
        zone.insert("SpaceHeatSystem".into(), system_name.into());

        Ok(self)
    }

    pub(crate) fn zone_has_space_cool_system(&self, zone: &str) -> JsonAccessResult<bool> {
        Ok(self.specific_zone(zone)?.get("SpaceCoolSystem").is_some())
    }

    pub(crate) fn space_cool_system_for_zone(
        &self,
        zone: &str,
    ) -> JsonAccessResult<Vec<smartstring::alias::String>> {
        Ok(match self.specific_zone(zone)?.get("SpaceCoolSystem") {
            Some(JsonValue::String(system)) => vec![smartstring::alias::String::from(system)],
            Some(JsonValue::Array(systems)) => systems
                .iter()
                .map(|s| {
                    Ok(smartstring::alias::String::from(s.as_str().ok_or(
                        json_error("SpaceCoolSystem list contained non-strings"),
                    )?))
                })
                .collect::<Result<_, _>>()?,
            _ => vec![],
        })
    }

    pub fn set_space_cool_system_for_zone(
        &mut self,
        zone: &str,
        system_name: &str,
    ) -> anyhow::Result<&Self> {
        let zone = self.specific_zone_mut(zone)?;
        zone.insert("SpaceCoolSystem".into(), system_name.into());

        Ok(self)
    }

    pub fn set_lighting_efficacy_for_all_zones(
        &mut self,
        efficacy: f64,
    ) -> JsonAccessResult<&Self> {
        for lighting in self
            .zone_node_mut()?
            .values_mut()
            .filter_map(|zone| zone.get_mut("Lighting").and_then(|l| l.as_object_mut()))
        {
            lighting.insert("efficacy".into(), efficacy.into());
        }

        Ok(self)
    }

    pub fn all_zones_have_bulbs(&self) -> JsonAccessResult<bool> {
        Ok(self.zone_node()?.values().all(|zone| {
            zone.get("Lighting")
                .and_then(|l| l.as_object())
                .is_some_and(|l| l.contains_key("bulbs"))
        }))
    }

    pub(crate) fn all_bulbs_mut(&mut self) -> JsonAccessResult<Vec<&mut serde_json::Value>> {
        Ok(self
            .zone_node_mut()?
            .values_mut()
            .map(|value| {
                value
                    .get_mut("Lighting")
                    .ok_or_else(|| json_error("Lighting not found"))?
                    .get_mut("bulbs")
                    .ok_or_else(|| json_error("Bulbs not found"))?
                    .as_array_mut()
                    .ok_or_else(|| json_error("Bulbs not an array"))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect())
    }

    pub fn light_bulbs_for_each_zone(
        &self,
    ) -> JsonAccessResult<IndexMap<smartstring::alias::String, Vec<JsonValue>>> {
        Ok(self
            .zone_node()?
            .iter()
            .map(|(zone_name, zone)| {
                let bulbs = zone
                    .get("Lighting")
                    .and_then(|lighting| lighting.get("bulbs"))
                    .and_then(|bulbs| bulbs.as_array());
                (
                    smartstring::alias::String::from(zone_name),
                    bulbs.map(ToOwned::to_owned).unwrap_or_default(),
                )
            })
            .collect())
    }

    #[expect(unused)]
    pub fn set_control_window_opening_for_zone(
        &mut self,
        zone: &str,
        opening_type: Option<&str>,
    ) -> anyhow::Result<&Self> {
        self.specific_zone_mut(zone)?
            .insert("Control_WindowOpening".into(), json!(opening_type));

        Ok(self)
    }

    pub fn set_control_string_for_space_heat_system(
        &mut self,
        space_heat_system: &str,
        control_string: &str,
    ) -> anyhow::Result<&Self> {
        self.root_object_mut("SpaceHeatSystem")?
            .get_mut(space_heat_system)
            .ok_or(anyhow!(
                "There is no provided space heat system with the name '{space_heat_system}'"
            ))?
            .as_object_mut()
            .ok_or(json_error("Space heat system was not an object"))?
            .insert("Control".into(), json!(control_string));

        Ok(self)
    }

    pub fn set_control_charger_for_space_heat_system(
        &mut self,
        space_heat_system: &str,
        control_string: &str,
    ) -> anyhow::Result<&Self> {
        self.root_object_mut("SpaceHeatSystem")?
            .get_mut(space_heat_system)
            .ok_or(anyhow!(
                "There is no provided space heat system with the name '{space_heat_system}'"
            ))?
            .as_object_mut()
            .ok_or(json_error("Space heat system was not an object"))?
            .insert("ControlCharger".into(), json!(control_string));

        Ok(self)
    }

    pub fn set_control_string_for_space_cool_system(
        &mut self,
        space_cool_system: &str,
        control_string: &str,
    ) -> anyhow::Result<&Self> {
        self.root_object_mut("SpaceCoolSystem")?
            .get_mut(space_cool_system)
            .ok_or(anyhow!(
                "There is no provided space cool system with the name '{space_cool_system}'"
            ))?
            .as_object_mut()
            .ok_or(json_error("Space cool system was not an object"))?
            .insert("Control".into(), json!(control_string));

        Ok(self)
    }

    pub(crate) fn set_efficiency_for_all_space_cool_systems(
        &mut self,
        efficiency: f64,
    ) -> JsonAccessResult<()> {
        let systems = self.root_object_entry_mut("SpaceCoolSystem")?;
        for system in systems.values_mut().flat_map(|s| s.as_object_mut()) {
            system.insert("efficiency".into(), json!(efficiency));
        }

        Ok(())
    }

    pub(crate) fn set_frac_convective_for_all_space_cool_systems(
        &mut self,
        frac_convective: f64,
    ) -> JsonAccessResult<()> {
        let systems = self.root_object_entry_mut("SpaceCoolSystem")?;
        for system in systems.values_mut().flat_map(|s| s.as_object_mut()) {
            system.insert("frac_convective".into(), json!(frac_convective));
        }

        Ok(())
    }

    pub(crate) fn set_energy_supply_for_all_space_cool_systems(
        &mut self,
        energy_supply_name: &str,
    ) -> JsonAccessResult<()> {
        let systems = self.root_object_entry_mut("SpaceCoolSystem")?;
        for system in systems.values_mut().flat_map(|s| s.as_object_mut()) {
            system.insert("EnergySupply".into(), json!(energy_supply_name));
        }

        Ok(())
    }

    #[cfg(test)]
    fn remove_custom_energy_supplies(&mut self) -> JsonAccessResult<()> {
        self.root_object_mut("EnergySupply")?
            .retain(|_, energy_supply| match energy_supply.get("fuel") {
                Some(fuel) if fuel.is_string() => fuel.as_str().unwrap() != "custom",
                _ => false,
            });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn space_cool_system(
        &self,
    ) -> JsonAccessResult<Option<&Map<std::string::String, JsonValue>>> {
        self.optional_root_object("SpaceCoolSystem")
    }

    pub(crate) fn space_heat_system_keys(
        &self,
    ) -> JsonAccessResult<Vec<smartstring::alias::String>> {
        Ok(match self.optional_root_object("SpaceHeatSystem")? {
            Some(space_heat_system) => space_heat_system
                .keys()
                .map(smartstring::alias::String::from)
                .collect(),
            None => vec![],
        })
    }

    pub fn space_heat_systems_mut(&mut self) -> JsonAccessResult<&mut Map<String, JsonValue>> {
        self.root_object_mut("SpaceHeatSystem")
    }

    pub fn temperature_setback_for_space_cool_system(
        &self,
        space_cool_system: &str,
    ) -> JsonAccessResult<Option<f64>> {
        let space_cool_systems = self.optional_root_object("SpaceCoolSystem")?;
        let space_cool_systems = match space_cool_systems {
            Some(ref space_heat_systems) => space_heat_systems,
            None => return Ok(None),
        };
        let space_cool_system = space_cool_systems.get(space_cool_system);

        Ok(space_cool_system.and_then(|space_cool_system| {
            space_cool_system.as_object().and_then(|space_cool_system| {
                space_cool_system
                    .get("temp_setback")
                    .and_then(|temp_setback| temp_setback.as_f64())
            })
        }))
    }

    pub fn advanced_start_for_space_cool_system(
        &self,
        space_cool_system: &str,
    ) -> JsonAccessResult<Option<f64>> {
        let space_cool_systems = self.optional_root_object("SpaceCoolSystem")?;
        let space_cool_systems = match space_cool_systems {
            Some(ref space_heat_systems) => space_heat_systems,
            None => return Ok(None),
        };
        let space_cool_system = space_cool_systems.get(space_cool_system);

        Ok(space_cool_system.and_then(|space_cool_system| {
            space_cool_system.as_object().and_then(|space_cool_system| {
                space_cool_system
                    .get("advanced_start")
                    .and_then(|temp_setback| temp_setback.as_f64())
            })
        }))
    }

    pub(crate) fn heat_source_for_space_heat_system(
        &self,
        space_heat_system: &str,
    ) -> JsonAccessResult<Option<&JsonValue>> {
        let space_heat_systems = self.optional_root_object("SpaceHeatSystem")?;
        let space_heat_systems = match space_heat_systems {
            Some(ref space_heat_systems) => space_heat_systems,
            None => return Ok(None),
        };
        let space_heat_system = space_heat_systems.get(space_heat_system);

        Ok(space_heat_system.and_then(|space_heat_system| {
            space_heat_system
                .as_object()
                .and_then(|space_heat_system| space_heat_system.get("HeatSource"))
        }))
    }

    pub(crate) fn set_hot_water_source(
        &mut self,
        hot_water_source: JsonValue,
    ) -> JsonAccessResult<&mut Self> {
        self.set_on_root_key("HotWaterSource", hot_water_source)
    }

    pub fn hot_water_source(&self) -> JsonAccessResult<&Map<std::string::String, JsonValue>> {
        self.root_object("HotWaterSource")
    }

    pub fn hot_water_source_mut(
        &mut self,
    ) -> JsonAccessResult<&mut Map<std::string::String, JsonValue>> {
        self.root_object_mut("HotWaterSource")
    }

    pub fn names_of_energy_supplies_with_diverters(
        &self,
    ) -> JsonAccessResult<Vec<smartstring::alias::String>> {
        Ok(self
            .root_object("EnergySupply")?
            .iter()
            .filter_map(|(energy_supply_name, energy_supply)| {
                energy_supply.as_object().and_then(|energy_supply| {
                    energy_supply
                        .get("diverter")
                        .map(|_| smartstring::alias::String::from(energy_supply_name))
                })
            })
            .collect_vec())
    }

    pub fn set_control_max_name_for_energy_supply_diverter(
        &mut self,
        energy_supply_name: &str,
        control_max_name: &str,
    ) -> JsonAccessResult<&Self> {
        self.root_object_mut("EnergySupply")?
            .get_mut(energy_supply_name)
            .ok_or_else(|| {
                json_error(format!(
                    "There is no provided energy supply with the name '{energy_supply_name}'"
                ))
            })?
            .as_object_mut()
            .ok_or_else(|| json_error("Energy supply was not an object"))?
            .get_mut("diverter")
            .ok_or_else(|| json_error("Diverter field not found on energy supply"))?
            .as_object_mut()
            .ok_or_else(|| json_error("Energy supply diverter is not an object"))?
            .insert("Controlmax".into(), json!(control_max_name));

        Ok(self)
    }

    pub fn set_lighting_gains(&mut self, gains_details: JsonValue) -> JsonAccessResult<&Self> {
        self.set_gains_for_field("lighting", gains_details)
    }

    pub fn set_topup_gains(&mut self, gains_details: JsonValue) -> JsonAccessResult<&Self> {
        self.set_gains_for_field("topup", gains_details)
    }

    pub fn set_gains_for_field(
        &mut self,
        field: impl Into<std::string::String>,
        gains_details: JsonValue,
    ) -> JsonAccessResult<&Self> {
        self.root_object_entry_mut("ApplianceGains")?
            .insert(field.into(), gains_details);

        Ok(self)
    }

    pub fn clear_appliance_gains(&mut self) -> JsonAccessResult<&mut Self> {
        self.set_on_root_key("ApplianceGains", json!({}))
    }

    pub fn set_priority_for_gains_appliance(
        &mut self,
        priority: isize,
        appliance: &str,
    ) -> anyhow::Result<()> {
        self.root_object_entry_mut("ApplianceGains")?
            .get_mut(appliance)
            .ok_or_else(|| anyhow!("Encountered bad appliance gains reference {appliance:?}"))?
            .as_object_mut()
            .ok_or_else(|| anyhow!("Appliance gains reference was not a JSON object"))?
            .insert("priority".into(), json!(priority));

        Ok(())
    }

    pub fn fuel_type_for_energy_supply_reference(
        &self,
        reference: &str,
    ) -> anyhow::Result<smartstring::alias::String> {
        Ok(self
            .root_object("EnergySupply")?
            .get(reference)
            .ok_or(anyhow!(
                "Energy supply with reference '{reference}' could not be found"
            ))?
            .get("fuel")
            .ok_or(json_error("Energy supply object did not have a fuel field"))?
            .as_str()
            .ok_or(json_error(
                "Energy supply fuel field expected to have a string value",
            ))?
            .into())
    }

    pub(crate) fn shower_flowrates(
        &self,
    ) -> JsonAccessResult<IndexMap<smartstring::alias::String, MaybeShowerFlowRateFields>> {
        let showers = match self
            .hot_water_demand()?
            .get("Shower")
            .and_then(|s| s.as_object())
        {
            None => return Ok(Default::default()),
            Some(showers) => showers,
        };

        Ok(showers
            .iter()
            .map(|(name, shower)| {
                let flowrate = shower.get("flowrate").and_then(|f| f.as_f64());
                let allow_low_flowrate = shower.get("allow_low_flowrate").and_then(|a| a.as_bool());
                (
                    smartstring::alias::String::from(name),
                    (flowrate, allow_low_flowrate),
                )
            })
            .collect())
    }

    pub fn reset_water_heating_events(&mut self) -> JsonAccessResult<&mut Self> {
        self.set_on_root_key("Events", json!({}))
    }

    pub(crate) fn showers(&self) -> JsonAccessResult<Option<&Map<std::string::String, JsonValue>>> {
        self.hot_water_demand()?
            .get("Shower")
            .map(|showers| {
                showers
                    .as_object()
                    .ok_or(json_error("Shower was not an object"))
            })
            .transpose()
    }

    pub fn shower_keys(&self) -> JsonAccessResult<Vec<smartstring::alias::String>> {
        Ok(self
            .hot_water_demand()?
            .get("Shower")
            .map_or(vec![], |showers| match showers.as_object() {
                Some(showers) => showers
                    .keys()
                    .map(smartstring::alias::String::from)
                    .collect(),
                None => vec![],
            }))
    }

    pub fn shower_name_refers_to_instant_electric(&self, name: &str) -> bool {
        self.hot_water_demand()
            .ok()
            .and_then(|demand| demand.get("Shower"))
            .and_then(|showers| showers.get(name))
            .and_then(|shower| shower.get("type"))
            .and_then(|shower_type| shower_type.as_str())
            .is_some_and(|shower_type| shower_type == "InstantElecShower")
    }

    pub(crate) fn shower_values_mut(&mut self) -> JsonAccessResult<Option<Vec<&mut JsonValue>>> {
        Ok(self
            .hot_water_demand_mut()?
            .get_mut("Shower")
            .and_then(|shower_node| shower_node.as_object_mut())
            .map(|shower| shower.values_mut().collect::<Vec<&mut JsonValue>>()))
    }

    pub(crate) fn register_wwhrs_name_on_showers(
        &mut self,
        wwhrs: &str,
        wwhrs_configuration: &str,
    ) -> anyhow::Result<()> {
        let showers = self
            .shower_values_mut()?
            .ok_or_else(|| anyhow!("Could not get shower values"))?;

        for shower in showers {
            let shower_object = shower
                .as_object_mut()
                .ok_or_else(|| anyhow!("Shower entry is not an object"))?;

            shower_object.insert("WWHRS".into(), json!(wwhrs));
            shower_object.insert("WWHRS_configuration".into(), json!(wwhrs_configuration));
        }

        Ok(())
    }

    pub(crate) fn baths(&self) -> JsonAccessResult<Option<&Map<std::string::String, JsonValue>>> {
        Ok(self
            .hot_water_demand()?
            .get("Bath")
            .and_then(|baths| baths.as_object()))
    }

    pub(crate) fn baths_mut(&mut self) -> JsonAccessResult<Option<&mut Map<String, JsonValue>>> {
        Ok(self
            .hot_water_demand_mut()?
            .get_mut("Bath")
            .and_then(|baths| baths.as_object_mut()))
    }

    pub fn bath_keys(&self) -> JsonAccessResult<Vec<smartstring::alias::String>> {
        Ok(self
            .hot_water_demand()?
            .get("Bath")
            .map_or(vec![], |baths| match baths.as_object() {
                Some(baths) => baths.keys().map(smartstring::alias::String::from).collect(),
                None => vec![],
            }))
    }

    pub fn size_for_bath_field(&self, field: &str) -> JsonAccessResult<Option<f64>> {
        Ok(self
            .hot_water_demand()?
            .get("Bath")
            .and_then(|baths| baths.as_object())
            .and_then(|bath| bath.get(field))
            .and_then(|bath| bath.get("size"))
            .and_then(|size| size.as_f64()))
    }

    pub fn flowrate_for_bath_field(&self, field: &str) -> JsonAccessResult<Option<f64>> {
        Ok(self
            .hot_water_demand()?
            .get("Bath")
            .and_then(|baths| baths.as_object())
            .and_then(|bath| bath.get(field))
            .and_then(|bath| bath.get("flowrate"))
            .and_then(|flowrate| flowrate.as_f64()))
    }

    pub(crate) fn other_water_uses(
        &self,
    ) -> JsonAccessResult<Option<&Map<std::string::String, JsonValue>>> {
        Ok(self
            .hot_water_demand()?
            .get("Other")
            .and_then(|other| other.as_object()))
    }

    pub(crate) fn other_water_uses_mut(
        &mut self,
    ) -> JsonAccessResult<Option<&mut Map<String, JsonValue>>> {
        Ok(self
            .hot_water_demand_mut()?
            .get_mut("Other")
            .and_then(|other| other.as_object_mut()))
    }

    pub fn other_water_use_keys(&self) -> JsonAccessResult<Vec<smartstring::alias::String>> {
        Ok(self
            .other_water_uses()?
            .map(|other| other.keys().map(smartstring::alias::String::from).collect())
            .unwrap_or(vec![]))
    }

    pub fn flow_rate_for_other_water_use_field(
        &self,
        field: &str,
    ) -> JsonAccessResult<Option<f64>> {
        Ok(self
            .hot_water_demand()?
            .get("Other")
            .and_then(|others| others.as_object())
            .and_then(|other| other.get(field))
            .and_then(|other| other.get("flowrate"))
            .and_then(|flowrate| flowrate.as_f64()))
    }

    pub fn set_other_water_use_details(
        &mut self,
        cold_water_source_type: &str,
        flowrate: f64,
    ) -> JsonAccessResult<()> {
        let other_details = json!({
            "flowrate": flowrate,
            "ColdWaterSource": cold_water_source_type,
        });

        let other_water_uses = self
            .hot_water_demand_mut()?
            .entry("Other")
            .or_insert(json!({}))
            .as_object_mut()
            .ok_or(json_error("Other water uses not provided as an object"))?;
        other_water_uses.insert("other".into(), other_details);

        Ok(())
    }

    pub(crate) fn water_distribution(&self) -> anyhow::Result<Option<WaterDistribution>> {
        Ok(self
            .root_object("HotWaterDemand")?
            .get("Distribution")
            .map(|node| serde_json::from_value::<WaterDistribution>(node.to_owned()))
            .transpose()?)
    }

    pub fn part_g_compliance(&self) -> JsonAccessResult<Option<bool>> {
        self.root()?
            .get("PartGcompliance")
            .map(|node| {
                node.as_bool()
                    .ok_or(json_error("Part G compliance was not passed as a boolean"))
            })
            .transpose()
    }

    pub fn set_part_g_compliance(&mut self, is_compliant: bool) -> JsonAccessResult<&Self> {
        self.root_mut()?
            .insert("PartGcompliance".into(), is_compliant.into());

        Ok(self)
    }

    pub fn add_water_heating_event(
        &mut self,
        event_type: &str,
        subtype_name: &str,
        event: JsonValue,
    ) -> JsonAccessResult<&Self> {
        let node_for_type = self
            .root_object_entry_mut("Events")?
            .entry(event_type)
            .or_insert(json!({}))
            .as_object_mut()
            .ok_or(json_error("Events node was not an object"))?;
        let node_for_subtype = node_for_type.entry(subtype_name).or_insert(json!([]));
        if let Some(events) = node_for_subtype.as_array_mut() {
            events.push(event);
        } else {
            return Err(json_error(format!(
                "Events node at '{event_type}' -> '{subtype_name}' was not an array"
            )));
        }

        Ok(self)
    }

    #[cfg(test)]
    fn water_heating_events_of_types(
        &self,
        event_types: &[&str],
    ) -> JsonAccessResult<Vec<JsonValue>> {
        Ok(self
            .root_object("Events")?
            .iter()
            .filter(|(event_type, _)| event_types.contains(&&***event_type))
            .flat_map(|(_, events)| {
                events
                    .as_object()
                    .map(|events| events.values().filter_map(JsonValue::as_array))
            })
            .flatten()
            .flatten()
            .cloned()
            .collect_vec())
    }

    pub fn cold_water_source_has_header_tank(&self) -> JsonAccessResult<bool> {
        Ok(self
            .root_object("ColdWaterSource")?
            .contains_key("header tank"))
    }

    pub fn set_cold_water_source_by_key(
        &mut self,
        key: &str,
        source_details: JsonValue,
    ) -> JsonAccessResult<&Self> {
        self.root_object_mut("ColdWaterSource")?
            .insert(key.into(), source_details);

        Ok(self)
    }

    pub fn set_hot_water_cylinder(&mut self, source_value: JsonValue) -> JsonAccessResult<&Self> {
        let hot_water_source = self.root_object_mut("HotWaterSource")?;
        hot_water_source.insert("hw cylinder".into(), source_value);

        Ok(self)
    }

    pub fn hot_water_demand(&self) -> JsonAccessResult<&Map<String, JsonValue>> {
        self.root_object("HotWaterDemand")
    }

    pub fn hot_water_demand_mut(&mut self) -> JsonAccessResult<&mut Map<String, JsonValue>> {
        self.root_object_mut("HotWaterDemand")
    }

    pub fn set_water_distribution(
        &mut self,
        distribution_value: JsonValue,
    ) -> JsonAccessResult<&Self> {
        self.hot_water_demand_mut()?
            .insert("Distribution".into(), distribution_value);

        Ok(self)
    }

    pub fn set_shower(&mut self, shower_value: JsonValue) -> anyhow::Result<&Self> {
        self.hot_water_demand_mut()?
            .insert("Shower".into(), shower_value);

        Ok(self)
    }

    pub fn set_bath(&mut self, bath_value: JsonValue) -> anyhow::Result<&Self> {
        self.hot_water_demand_mut()?
            .insert("Bath".into(), bath_value);

        Ok(self)
    }

    pub fn set_other_water_use(
        &mut self,
        other_water_use_value: JsonValue,
    ) -> anyhow::Result<&Self> {
        self.hot_water_demand_mut()?
            .insert("Other".into(), other_water_use_value);

        Ok(self)
    }

    pub fn remove_wwhrs(&mut self) -> JsonAccessResult<&mut Self> {
        self.remove_root_key("WWHRS")
    }

    pub(crate) fn wwhrs(&self) -> anyhow::Result<Option<WasteWaterHeatRecovery>> {
        Ok(self
            .root()?
            .get("WWHRS")
            .map(|wwhrs| match serde_json::from_value(wwhrs.to_owned()) {
                Ok(wwhrs) => Ok(wwhrs),
                Err(err) => Err(err),
            })
            .transpose()?)
    }

    pub(crate) fn set_wwhrs(&mut self, wwhrs: JsonValue) -> JsonAccessResult<&mut Self> {
        self.set_on_root_key("WWHRS", wwhrs)
    }

    pub fn remove_space_heat_systems(&mut self) -> JsonAccessResult<&mut Self> {
        self.remove_root_key("SpaceHeatSystem")
    }

    pub fn space_heat_system_for_key(&self, key: &str) -> JsonAccessResult<Option<&JsonValue>> {
        Ok(self.root_object("SpaceHeatSystem")?.get(key))
    }

    pub fn set_space_heat_system_for_key(
        &mut self,
        key: &str,
        space_heat_system_value: JsonValue,
    ) -> JsonAccessResult<&Self> {
        self.root_object_entry_mut("SpaceHeatSystem")?
            .insert(key.into(), space_heat_system_value);
        Ok(self)
    }

    pub fn remove_space_cool_systems(&mut self) -> JsonAccessResult<&mut Self> {
        self.remove_root_key("SpaceCoolSystem")
    }

    pub fn remove_space_cool_systems_for_all_zones(&mut self) -> JsonAccessResult<&mut Self> {
        self.zone_node_mut()?.values_mut().for_each(|zone| {
            let zone = match zone.as_object_mut() {
                None => return,
                Some(zone) => zone,
            };
            zone.remove("SpaceCoolSystem");
        });
        Ok(self)
    }

    pub fn set_space_cool_system_for_key(
        &mut self,
        key: &str,
        space_cool_system_value: JsonValue,
    ) -> JsonAccessResult<&Self> {
        self.root_object_entry_mut("SpaceCoolSystem")?
            .insert(key.into(), space_cool_system_value);
        Ok(self)
    }

    pub(crate) fn set_on_site_generation(
        &mut self,
        on_site_generation: JsonValue,
    ) -> JsonAccessResult<&mut Self> {
        self.set_on_root_key("OnSiteGeneration", on_site_generation)
    }

    pub fn remove_on_site_generation(&mut self) -> JsonAccessResult<&mut Self> {
        self.remove_root_key("OnSiteGeneration")
    }

    pub(crate) fn on_site_generation(
        &self,
    ) -> JsonAccessResult<Option<&Map<std::string::String, JsonValue>>> {
        self.optional_root_object("OnSiteGeneration")
    }

    pub fn remove_all_diverters_from_energy_supplies(&mut self) -> JsonAccessResult<&mut Self> {
        self.root_object_entry_mut("EnergySupply")?
            .values_mut()
            .filter_map(|value| value.as_object_mut())
            .for_each(|energy_supply| {
                energy_supply.shift_remove("diverter");
            });
        Ok(self)
    }

    pub(crate) fn add_energy_supply_for_key(
        &mut self,
        energy_supply_key: &str,
        energy_supply_details: JsonValue,
    ) -> JsonAccessResult<()> {
        self.root_object_entry_mut("EnergySupply")?
            .insert(energy_supply_key.into(), energy_supply_details);

        Ok(())
    }

    pub(crate) fn energy_supply_by_key(
        &self,
        energy_supply_key: &str,
    ) -> JsonAccessResult<Option<&Map<std::string::String, JsonValue>>> {
        Ok(self
            .root_object("EnergySupply")?
            .get(energy_supply_key)
            .and_then(|energy_supply| energy_supply.as_object()))
    }

    pub fn energy_supplies_mut(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut Map<std::string::String, JsonValue>>> {
        Ok(self
            .root_object_mut("EnergySupply")?
            .values_mut()
            .filter_map(|value| value.as_object_mut())
            .collect::<Vec<&mut Map<String, JsonValue>>>())
    }

    pub(crate) fn energy_supplies_contain_key(
        &self,
        energy_supply_key: &str,
    ) -> JsonAccessResult<bool> {
        Ok(self
            .root_object("EnergySupply")?
            .contains_key(energy_supply_key))
    }

    #[cfg(test)]
    pub(crate) fn add_diverter_to_energy_supply(
        &mut self,
        energy_supply_key: &str,
        diverter: JsonValue,
    ) -> JsonAccessResult<()> {
        if let Some(energy_supply) = self
            .root_object_mut("EnergySupply")?
            .get_mut(energy_supply_key)
            .and_then(|energy_supply| energy_supply.as_object_mut())
        {
            energy_supply.insert("diverter".into(), diverter);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn add_electric_battery_to_energy_supply(
        &mut self,
        energy_supply_key: &str,
        electric_battery: JsonValue,
    ) -> JsonAccessResult<()> {
        if let Some(energy_supply) = self
            .root_object_mut("EnergySupply")?
            .get_mut(energy_supply_key)
            .and_then(|energy_supply| energy_supply.as_object_mut())
        {
            energy_supply.insert("ElectricBattery".into(), electric_battery);
        }

        Ok(())
    }

    pub fn remove_all_batteries_from_energy_supplies(&mut self) -> JsonAccessResult<&mut Self> {
        for energy_supply in self
            .root_object_mut("EnergySupply")?
            .values_mut()
            .flat_map(|v| v.as_object_mut())
        {
            energy_supply.shift_remove("ElectricBattery");
        }
        Ok(self)
    }

    pub fn external_conditions(&self) -> anyhow::Result<ExternalConditionsInput> {
        serde_json::from_value(
            self.root()?
                .get("ExternalConditions")
                .ok_or(json_error("ExternalConditions not found"))?
                .to_owned(),
        )
        .map_err(Into::into)
    }

    fn all_building_elements_of_types(
        &self,
        types: &[&str],
    ) -> JsonAccessResult<Vec<&Map<std::string::String, JsonValue>>> {
        let building_elements = self
            .zone_node()?
            .values()
            .filter_map(|zone| zone.get("BuildingElement")?.as_object())
            .flat_map(|obj| obj.values());
        let filtered_building_elements = building_elements.filter_map(|element| {
            let element_type = element.get("type")?.as_str()?;
            if types.contains(&element_type) {
                element.as_object()
            } else {
                None
            }
        });
        Ok(filtered_building_elements.collect())
    }

    fn all_building_elements_mut_of_types(
        &mut self,
        types: &[&str],
    ) -> JsonAccessResult<Vec<&mut Map<std::string::String, JsonValue>>> {
        Ok(self
            .zone_node_mut()?
            .values_mut()
            .filter_map(|zone| {
                zone.get_mut("BuildingElement")
                    .and_then(|building_element_node| building_element_node.as_object_mut())
            })
            .flat_map(|building_elements| building_elements.values_mut())
            .filter(|building_element| {
                building_element
                    .get("type")
                    .and_then(|building_element_type| building_element_type.as_str())
                    .is_some_and(|building_element_type| types.contains(&building_element_type))
            })
            .filter_map(|element| element.as_object_mut())
            .collect())
    }

    pub(crate) fn all_building_elements_mut(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut Map<std::string::String, JsonValue>>> {
        Ok(self
            .zone_node_mut()?
            .values_mut()
            .filter_map(|zone| {
                zone.get_mut("BuildingElement")
                    .and_then(|building_element_node| building_element_node.as_object_mut())
            })
            .flat_map(|building_elements| building_elements.values_mut())
            .filter_map(|element| element.as_object_mut())
            .collect())
    }

    pub fn all_transparent_building_elements_mut(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut Map<std::string::String, JsonValue>>> {
        self.all_building_elements_mut_of_types(&["BuildingElementTransparent"])
    }

    pub(crate) fn all_ground_building_elements_mut(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut Map<std::string::String, JsonValue>>> {
        self.all_building_elements_mut_of_types(&["BuildingElementGround"])
    }

    pub(crate) fn all_party_wall_building_elements_mut(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut Map<String, JsonValue>>> {
        self.all_building_elements_mut_of_types(&["BuildingElementPartyWall"])
    }

    pub(crate) fn all_opaque_and_adjztu_building_elements_mut_u_values(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut Map<std::string::String, JsonValue>>> {
        self.all_building_elements_mut_of_types(&[
            "BuildingElementOpaque",
            "BuildingElementAdjacentUnconditionedSpace_Simple",
        ])
    }

    pub(super) fn all_opaque_building_elements_except_unheated_pitched_roofs(
        &self,
    ) -> JsonAccessResult<Vec<&Map<std::string::String, JsonValue>>> {
        let opaque_building_elements =
            self.all_building_elements_of_types(&["BuildingElementOpaque"])?;

        let filtered_building_elements = opaque_building_elements.into_iter().filter(|element| {
            let is_unheated_pitched_roof = element
                .get("is_unheated_pitched_roof")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            !is_unheated_pitched_roof
        });
        let result = filtered_building_elements.collect();

        Ok(result)
    }

    fn base_height_of_building_element(
        building_element: &Map<String, JsonValue>,
    ) -> JsonAccessResult<f64> {
        building_element
            .get("base_height")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                json_error("Base height for opaque building element not found or not a number")
            })
    }

    fn height_of_building_element(
        building_element: &Map<String, JsonValue>,
    ) -> JsonAccessResult<f64> {
        building_element
            .get("height")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                json_error("Height for opaque building element not found or not a number")
            })
    }

    pub(super) fn habitable_building_height(&self) -> JsonAccessResult<f64> {
        let building_elements =
            self.all_opaque_building_elements_except_unheated_pitched_roofs()?;

        let mut base_heights: Vec<f64> = Vec::new();
        let mut total_heights: Vec<f64> = Vec::new();
        for element in building_elements {
            let base_height = Self::base_height_of_building_element(element)?;
            base_heights.push(base_height);
            let height = Self::height_of_building_element(element)?;
            total_heights.push(height + base_height);
        }

        let max_total_height = total_heights.iter().max_by(|a, b| a.total_cmp(b)).ok_or_else(|| json_error("Expected opaque building elements that are not unheated pitched roofs to exist and have heights"));
        let min_base_height = base_heights.iter().min_by(|a, b| a.total_cmp(b)).ok_or_else(|| json_error("Expected opaque building elements that are not unheated pitched roofs to exist and have base heights"));

        Ok(max_total_height? - min_base_height?)
    }

    pub(crate) fn set_numeric_field_for_building_element(
        &mut self,
        building_element_reference: &str,
        field: &str,
        value: f64,
    ) -> anyhow::Result<()> {
        *self.zone_node_mut()?
            .values_mut()
            .filter_map(|zone| zone.get_mut("BuildingElement").and_then(|el| el.as_object_mut()))
            .flatten()
            .find(|(name, _value)| *name == building_element_reference)
            .ok_or(anyhow!("Could not find building element with reference '{building_element_reference}'"))?
            .1
            .get_mut(field)
            .ok_or(anyhow!("Could not find field '{field}' on building element with reference '{building_element_reference}'"))? = json!(value);

        Ok(())
    }

    pub fn all_building_elements(
        &self,
    ) -> JsonAccessResult<IndexMap<smartstring::alias::String, &JsonValue>> {
        self.zone_node()?
            .values()
            .filter_map(|zone| zone.get("BuildingElement").and_then(|el| el.as_object()))
            .flatten()
            .map(|(key, el)| Ok((smartstring::alias::String::from(key), el)))
            .collect()
    }

    pub(crate) fn all_building_element_values(&self) -> JsonAccessResult<Vec<&JsonValue>> {
        Ok(self
            .zone_node()?
            .values()
            .filter_map(|zone| zone.get("BuildingElement").and_then(|el| el.as_object()))
            .flatten()
            .map(|(_, el)| el)
            .collect())
    }

    pub(crate) fn all_building_element_values_mut(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut JsonValue>> {
        Ok(self
            .zone_node_mut()?
            .values_mut()
            .filter_map(|zone| {
                zone.get_mut("BuildingElement")
                    .and_then(|el| el.as_object_mut())
            })
            .flatten()
            .map(|(_, el)| el)
            .collect())
    }

    pub fn all_energy_supply_fuel_types(
        &self,
    ) -> JsonAccessResult<HashSet<smartstring::alias::String>> {
        let mut fuel_types = HashSet::new();
        for fuel in self
            .root_object("EnergySupply")?
            .values()
            .flat_map(|supply| supply.get("fuel").map(|fuel| fuel.as_str()))
            .flatten()
        {
            fuel_types.insert(smartstring::alias::String::from(fuel));
        }

        Ok(fuel_types)
    }

    pub fn has_appliances(&self) -> JsonAccessResult<bool> {
        Ok(self.root()?.contains_key("Appliances"))
    }

    pub fn merge_in_appliances(
        &mut self,
        appliances: &IndexMap<&str, JsonValue>,
    ) -> anyhow::Result<()> {
        let mut appliances_value = serde_json::to_value(appliances.to_owned())?;
        let appliances = appliances_value
            .as_object_mut()
            .ok_or(anyhow!("Appliances were not an object when expected to be"))?;
        let existing_appliances = self.root_object_entry_mut("Appliances")?;
        existing_appliances.append(appliances);

        Ok(())
    }

    pub fn remove_appliance(&mut self, appliance_key: &str) -> JsonAccessResult<&Self> {
        // we use .shift_remove instead of remove here
        // to preserve the relative order of the appliances
        self.root_object_entry_mut("Appliances")?
            .shift_remove(appliance_key);

        Ok(self)
    }

    pub fn appliances_contain_key(&self, name: &str) -> bool {
        self.root_object("Appliances")
            .ok()
            .is_some_and(|appliances| appliances.contains_key(name))
    }

    pub fn appliance_key_has_reference(
        &self,
        key: &str,
        reference: &str,
    ) -> JsonAccessResult<bool> {
        let empty_map = Map::new();
        Ok(self
            .root_object("Appliances")
            .unwrap_or(&empty_map)
            .get(key)
            .and_then(|value| value.as_str())
            .is_some_and(|appliance_reference| appliance_reference == reference))
    }

    pub fn appliance_with_key(&self, key: &str) -> JsonAccessResult<Option<&JsonValue>> {
        Ok(match self.root_object("Appliances") {
            Err(_) => return Ok(None),
            Ok(appliances) => appliances.get(key),
        })
    }

    pub(crate) fn appliance_with_key_mut(
        &mut self,
        key: &str,
    ) -> JsonAccessResult<Option<&mut JsonValue>> {
        Ok(self.root_object_entry_mut("Appliances")?.get_mut(key))
    }

    pub fn clone_appliances(&self) -> Map<std::string::String, JsonValue> {
        self.root_object("Appliances")
            .cloned()
            .unwrap_or(Map::new())
    }

    pub fn energy_supply_for_appliance(&self, key: &str) -> anyhow::Result<&str> {
        let appliances = self.root_object("Appliances")?;

        appliances
            .get(key)
            .and_then(|appliance| appliance.as_object())
            .ok_or_else(|| anyhow!("No {key} object in appliances input"))?
            .get("Energysupply")
            .and_then(|supply| supply.as_str())
            .ok_or_else(|| anyhow!("No energy supply for appliance '{key}'"))
    }

    pub fn loadshifting_for_appliance(
        &self,
        appliance_key: &str,
    ) -> JsonAccessResult<Option<Map<std::string::String, JsonValue>>> {
        let appliance = self.appliance_with_key(appliance_key)?;

        Ok(appliance
            .and_then(|appliance| appliance.get("loadshifting"))
            .and_then(|load_shifting| load_shifting.as_object())
            .cloned())
    }

    pub fn set_loadshifting_for_appliance(
        &mut self,
        appliance_key: &str,
        new_load_shifting: JsonValue,
    ) -> JsonAccessResult<()> {
        let mut appliance = self.appliance_with_key_mut(appliance_key)?;
        if let Some(appliance) = appliance
            .as_mut()
            .and_then(|appliance| appliance.as_object_mut())
        {
            appliance.insert("loadshifting".into(), new_load_shifting);
        }

        Ok(())
    }

    pub fn infiltration_ventilation_node(&self) -> JsonAccessResult<&JsonValue> {
        self.root()?
            .get("InfiltrationVentilation")
            .ok_or_else(|| json_error("InfiltrationVentilation node not found"))
    }

    pub fn infiltration_ventilation_node_mut(
        &mut self,
    ) -> JsonAccessResult<&mut Map<std::string::String, JsonValue>> {
        self.root_object_mut("InfiltrationVentilation")
    }

    pub fn set_cross_vent_possible(
        &mut self,
        cross_vent_possible: bool,
    ) -> JsonAccessResult<&Self> {
        self.infiltration_ventilation_node_mut()?
            .insert("cross_vent_possible".into(), cross_vent_possible.into());
        Ok(self)
    }

    pub fn mechanical_ventilations_for_processing(
        &mut self,
    ) -> JsonAccessResult<Vec<&mut Map<std::string::String, JsonValue>>> {
        let mech_vents = match self
            .infiltration_ventilation_node_mut()?
            .get_mut("MechanicalVentilation")
            .and_then(|v| v.as_object_mut())
        {
            None => return Ok(Vec::new()),
            Some(mech_vents) => mech_vents,
        };
        Ok(mech_vents
            .values_mut()
            .filter_map(|v| v.as_object_mut())
            .collect())
    }

    pub fn set_mechanical_ventilations(
        &mut self,
        mech_vents: JsonValue,
    ) -> JsonAccessResult<&Self> {
        let infiltration_ventilation_node = self
            .input
            .get_mut("InfiltrationVentilation")
            .ok_or(json_error("InfiltrationVentilation node not found"))?
            .as_object_mut()
            .ok_or(json_error("InfiltrationVentilation node is not an object"))?;
        infiltration_ventilation_node.insert("MechanicalVentilation".into(), mech_vents);

        Ok(self)
    }

    pub fn set_vents(&mut self, mech_vents: JsonValue) -> JsonAccessResult<&Self> {
        let infiltration_ventilation_node = self
            .input
            .get_mut("InfiltrationVentilation")
            .ok_or(json_error("InfiltrationVentilation node not found"))?
            .as_object_mut()
            .ok_or(json_error("InfiltrationVentilation node is not an object"))?;
        infiltration_ventilation_node.insert("Vents".into(), mech_vents);

        Ok(self)
    }

    pub fn vents_mut(&mut self) -> JsonAccessResult<&mut Map<std::string::String, JsonValue>> {
        self.root_object_mut("InfiltrationVentilation")?
            .get_mut("Vents")
            .and_then(|v| v.as_object_mut())
            .ok_or(json_error("Vents node not available"))
    }

    pub fn set_control_for_mechanical_ventilation(
        &mut self,
        mech_vent_key: &str,
        control: &str,
    ) -> JsonAccessResult<&Self> {
        let infiltration_ventilation = self.infiltration_ventilation_node_mut()?;
        let mech_vent_map = infiltration_ventilation
            .get_mut("MechanicalVentilation")
            .ok_or(json_error("MechanicalVentilation node not found"))?
            .as_object_mut()
            .ok_or(json_error("MechanicalVentilation node is not an object"))?;
        let mech_vent = mech_vent_map
            .get_mut(mech_vent_key)
            .and_then(JsonValue::as_object_mut)
            .ok_or(json_error(format!(
                "Mechanical ventilation '{mech_vent_key}' not found"
            )))?;
        mech_vent.insert("Control".into(), json!(control));

        Ok(self)
    }

    #[cfg(test)]
    pub(crate) fn mechanical_ventilation_control_by_key(
        &self,
        mech_vent_key: &str,
    ) -> JsonAccessResult<&JsonValue> {
        let infiltration_ventilation_node = self
            .input
            .get("InfiltrationVentilation")
            .ok_or(json_error("InfiltrationVentilation node not found"))?
            .as_object()
            .ok_or(json_error("InfiltrationVentilation node is not an object"))?;
        let mech_vent_map = infiltration_ventilation_node
            .get("MechanicalVentilation")
            .ok_or(json_error("MechanicalVentilation node not found"))?
            .as_object()
            .ok_or(json_error("MechanicalVentilation node is not an object"))?;
        let mech_vent = mech_vent_map
            .get(mech_vent_key)
            .ok_or(json_error(format!(
                "No MechanicalVentilation with key {}",
                mech_vent_key
            )))?
            .as_object()
            .ok_or(json_error(format!(
                "MechanicalVentilation {} is not an object",
                mech_vent_key
            )))?;
        let mech_vent_control = mech_vent.get("Control").ok_or(json_error(format!(
            "Control is not on MechanicalVentilation object {}",
            mech_vent_key
        )))?;
        Ok(mech_vent_control)
    }

    pub fn set_window_adjust_control_for_infiltration_ventilation(
        &mut self,
        control: &str,
    ) -> JsonAccessResult<&Self> {
        self.infiltration_ventilation_node_mut()?
            .insert("Control_WindowAdjust".into(), control.into());
        Ok(self)
    }

    pub fn set_vent_adjust_min_control_for_infiltration_ventilation(
        &mut self,
        control: &str,
    ) -> JsonAccessResult<&Self> {
        self.infiltration_ventilation_node_mut()?
            .insert("Control_VentAdjustMin".into(), control.into());
        Ok(self)
    }

    pub fn set_vent_adjust_max_control_for_infiltration_ventilation(
        &mut self,
        control: &str,
    ) -> JsonAccessResult<&Self> {
        self.infiltration_ventilation_node_mut()?
            .insert("Control_VentAdjustMax".into(), control.into());
        Ok(self)
    }

    pub fn set_test_pressure_for_infiltration_ventilation_leaks(
        &mut self,
        test_pressure: f64,
    ) -> JsonAccessResult<()> {
        self.infiltration_ventilation_node_mut()?
            .get_mut("Leaks")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| json_error("Leaks node not found or not an object"))?
            .insert("test_pressure".into(), test_pressure.into());

        Ok(())
    }

    pub fn infiltration_ventilation_is_noise_nuisance(&self) -> bool {
        self.root_object("InfiltrationVentilation")
            .ok()
            .and_then(|infiltration| infiltration.get("noise_nuisance"))
            .and_then(|nuisance| nuisance.as_bool())
            .unwrap_or(false)
    }

    pub(crate) fn ventilation_zone_base_height(&self) -> JsonAccessResult<f64> {
        self.root_object("InfiltrationVentilation")?
            .get("ventilation_zone_base_height")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| json_error("ventilation_zone_base_height missing or not a number"))
    }

    pub(crate) fn set_heat_source_wet(
        &mut self,
        heat_source_wet: JsonValue,
    ) -> JsonAccessResult<()> {
        self.root_mut()?
            .insert("HeatSourceWet".into(), heat_source_wet);
        Ok(())
    }

    pub(crate) fn heat_source_wet(
        &self,
    ) -> anyhow::Result<IndexMap<smartstring::alias::String, JsonValue>> {
        self.root()?
            .get("HeatSourceWet")
            .and_then(|value| value.as_object())
            .into_iter()
            .flatten()
            .map(|(name, source)| {
                Ok((
                    smartstring::alias::String::from(name),
                    serde_json::from_value(source.clone())?,
                ))
            })
            .collect::<anyhow::Result<_, _>>()
    }

    pub fn heat_source_wet_mut(&mut self) -> JsonAccessResult<Option<&mut Map<String, JsonValue>>> {
        self.optional_root_object_mut("HeatSourceWet")
    }

    pub(crate) fn heat_source_wet_by_key(
        &self,
        key: &str,
    ) -> anyhow::Result<&Map<String, JsonValue>> {
        self.root()?
            .get("HeatSourceWet")
            .and_then(|value| value.get(key))
            .and_then(|value| value.as_object())
            .ok_or_else(|| anyhow!("No HeatSourceWet object with key {key}"))
    }

    pub(crate) fn heat_source_wet_by_key_mut(
        &mut self,
        key: &str,
    ) -> anyhow::Result<&mut Map<String, JsonValue>> {
        self.root_mut()?
            .get_mut("HeatSourceWet")
            .and_then(|value| value.get_mut(key))
            .and_then(|value| value.as_object_mut())
            .ok_or_else(|| anyhow!("No HeatSourceWet object with key {key}"))
    }

    pub fn remove_heat_source_wet(&mut self) -> JsonAccessResult<&mut Self> {
        self.remove_root_key("HeatSourceWet")
    }

    pub(crate) fn cold_water_source_name(&self) -> anyhow::Result<String> {
        let cold_water_sources = self
            .root()?
            .get("ColdWaterSource")
            .and_then(|obj| obj.as_object())
            .ok_or_else(|| anyhow!("No ColdWaterSource object present"))?;
        if cold_water_sources.len() != 1 {
            return Err(anyhow!(
                "Expected exactly one ColdWaterSource object, but found {}",
                cold_water_sources.len()
            ));
        }

        Ok(cold_water_sources.keys().next().unwrap().to_owned())
    }

    pub(crate) fn cold_water_source(&self) -> anyhow::Result<ColdWaterSourceInput> {
        Ok(serde_json::from_value(
            self.root()?
                .get("ColdWaterSource")
                .cloned()
                .ok_or(json_error("ColdWaterSource was not present"))?,
        )?)
    }

    #[cfg(test)]
    pub(crate) fn set_storeys_in_dwelling(&mut self, storeys: usize) -> JsonAccessResult<&Self> {
        self.root_object_mut("General")?
            .insert("storeys_in_dwelling".into(), json!(storeys));

        Ok(self)
    }

    pub(crate) fn storeys_in_dwelling(&self) -> JsonAccessResult<usize> {
        Ok(self
            .input
            .get("General")
            .ok_or(json_error("General node not found"))?
            .get("storeys_in_dwelling")
            .ok_or(json_error("storeys_in_dwelling field not found"))?
            .as_u64()
            .ok_or(json_error(
                "storeys_in_dwelling field is not a positive integer",
            ))? as usize)
    }

    pub(super) fn building_length(&self) -> JsonAccessResult<f64> {
        self.input
            .get("BuildingLength")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| json_error("Building length missing or not a number"))
    }

    pub(super) fn building_width(&self) -> JsonAccessResult<f64> {
        self.input
            .get("BuildingWidth")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| json_error("Building width missing or not a number"))
    }

    pub(crate) fn build_type(&self) -> JsonAccessResult<smartstring::alias::String> {
        Ok(self
            .root_object("General")?
            .get("build_type")
            .ok_or(json_error(
                "There was no build_type field on the General input object",
            ))?
            .as_str()
            .ok_or(json_error("The build_type field was not a string"))?
            .into())
    }

    pub(crate) fn storeys_in_building(&self) -> JsonAccessResult<Option<usize>> {
        self.input
            .get("General")
            .ok_or(json_error("General node not found"))?
            .get("storeys_in_building")
            .map(|v| {
                v.as_u64()
                    .ok_or(json_error("storeys_in_building is not a positive integer"))
                    .map(|n| n as usize)
            })
            .transpose()
    }

    pub(crate) fn hot_water_cylinder_volume(&self) -> JsonAccessResult<Option<f64>> {
        Ok(self
            .root_object("HotWaterSource")?
            .get("hw cylinder")
            .and_then(|cylinder| cylinder.get("volume"))
            .and_then(|v| v.as_f64()))
    }

    pub(crate) fn ground_floor_area(&self) -> JsonAccessResult<Option<f64>> {
        Ok(self
            .root()?
            .get("GroundFloorArea")
            .and_then(|area| area.as_f64()))
    }

    pub(crate) fn primary_pipework_clone(&self) -> anyhow::Result<Option<Vec<WaterPipework>>> {
        Ok(self
            .hot_water_source()?
            .get("hw cylinder")
            .and_then(|cylinder| cylinder.get("primary_pipework"))
            .and_then(|primary_pipework| primary_pipework.is_array().then_some(primary_pipework))
            .map(|primary_pipework| serde_json::from_value(primary_pipework.to_owned()))
            .transpose()?)
    }

    pub(crate) fn water_heating_event_by_type_and_name(
        &self,
        event_type: &str,
        event_name: &str,
    ) -> anyhow::Result<Option<Vec<WaterHeatingEvent>>> {
        let result = Ok(self
            .root_object("Events")?
            .get(event_type)
            .and_then(|event_group| event_group.get(event_name))
            .map(|events| serde_json::from_value(events.to_owned()))
            .transpose()?);

        if let Ok(None) = result {
            println!(
                "No events found for event type {} and name {}",
                event_type, event_name
            );
            println!("Events: {:?}", self.root_object("Events")?.keys());
        }

        result
    }

    pub(crate) fn part_o_active_cooling_required(&self) -> JsonAccessResult<Option<bool>> {
        Ok(match self.input.get("PartO_active_cooling_required") {
            None => None,
            Some(JsonValue::Bool(whether)) => Some(*whether),
            Some(_) => {
                return Err(json_error(
                    "PartO_active_cooling_required field not a boolean",
                ));
            }
        })
    }

    #[cfg(test)]
    pub fn set_part_o_active_cooling_required(
        &mut self,
        required: bool,
    ) -> JsonAccessResult<&mut Self> {
        self.set_on_root_key("PartO_active_cooling_required", json!(required))
    }

    #[cfg(test)]
    pub(crate) fn set_zone(&mut self, zone: JsonValue) -> JsonAccessResult<&mut Self> {
        self.set_on_root_key("Zone", zone)
    }

    pub fn remove_fhs_only_fields(&mut self) -> JsonAccessResult<&mut Self> {
        // this tracks logic from future_homes_standard.py (remove_fhs_only_inputs function)
        let top_level_keys_to_remove = [
            "Appliances",
            "General",
            "GroundFloorArea",
            "HeatingControlType",
            "NumberOfBedrooms",
            "NumberOfHabitableRooms",
            "NumberOfWetRooms",
            "NumberOfUtilityRooms",
            "NumberOfBathrooms",
            "NumberOfSanitaryAccommodations",
            "PartGcompliance",
            "PartO_active_cooling_required",
            "BuildingLength",
            "BuildingWidth",
            "NumberOfHotTappedRooms",
            "KitchenExtractorHoodExternal",
        ];
        {
            let root = self.root_mut()?;
            for key in top_level_keys_to_remove {
                root.remove(key);
            }
        }

        if let Ok(infiltration) = self.infiltration_ventilation_node_mut() {
            infiltration.remove("noise_nuisance");
        }
        if let Ok(vents) = self.mechanical_ventilations_for_processing() {
            for vent in vents {
                vent.remove("measured_fan_power");
                vent.remove("measured_air_flow_rate");
            }
        }
        if let Ok(heat_source_wet) = self.root_object_mut("HeatSourceWet") {
            for heat_source in heat_source_wet
                .values_mut()
                .filter_map(|v| v.as_object_mut())
            {
                heat_source.remove("is_heat_network");
                heat_source.remove("heat_network_type");
            }
        }
        if let Ok(space_heat_systems) = self.root_object_mut("SpaceHeatSystem") {
            for heat_system in space_heat_systems
                .values_mut()
                .filter_map(|v| v.as_object_mut())
            {
                heat_system.remove("advanced_start");
                heat_system.remove("temp_setback");
            }
        }
        if let Ok(space_cool_systems) = self.root_object_mut("SpaceCoolSystem") {
            for cool_system in space_cool_systems
                .values_mut()
                .filter_map(|v| v.as_object_mut())
            {
                cool_system.remove("advanced_start");
                cool_system.remove("temp_setback");
            }
        }
        if let Ok(Some(ref mut showers)) = self.shower_values_mut() {
            for shower in showers.iter_mut().filter_map(|v| v.as_object_mut()) {
                shower.remove("allow_low_flowrate");
            }
        }
        if let Ok(ref mut zones) = self.zone_node_mut() {
            for zone in zones.values_mut().filter_map(|v| v.as_object_mut()) {
                zone.remove("Lighting");
                zone.remove("livingroom_area");
                zone.remove("restofdwelling_area");
                if let Some(building_elements) = zone
                    .get_mut("BuildingElement")
                    .and_then(|be| be.as_object_mut())
                {
                    for building_element in building_elements
                        .values_mut()
                        .filter_map(|v| v.as_object_mut())
                    {
                        building_element.remove("security_risk");
                        building_element.remove("is_external_door");
                    }
                }
                if let Some(building_elements) = zone
                    .get_mut("ThermalBridging")
                    .and_then(|be| be.as_object_mut())
                {
                    for building_element in building_elements
                        .values_mut()
                        .filter_map(|v| v.as_object_mut())
                    {
                        building_element.remove("junction_type");
                    }
                }
            }
        }

        Ok(self)
    }
}

pub trait UValueEditableBuildingElement {
    fn set_u_value(&mut self, new_u_value: f64);
    fn pitch(&self) -> JsonAccessResult<f64>;
    fn is_opaque(&self) -> bool;
    fn is_external_door(&self) -> Option<bool>;
    fn remove_thermal_resistance_construction(&mut self);
}

pub struct UValueEditableBuildingElementJsonValue<'a>(
    pub &'a mut Map<std::string::String, JsonValue>,
);

impl UValueEditableBuildingElement for UValueEditableBuildingElementJsonValue<'_> {
    fn set_u_value(&mut self, new_u_value: f64) {
        self.0.insert("u_value".to_string(), json!(new_u_value));
    }

    fn pitch(&self) -> JsonAccessResult<f64> {
        self.0
            .get("pitch")
            .ok_or(json_error("Pitch field not provided"))?
            .as_f64()
            .ok_or(json_error("Pitch field did not provide number"))
    }

    fn is_opaque(&self) -> bool {
        self.0
            .get("type")
            .and_then(|v| v.as_str())
            .is_some_and(|building_type| building_type == "BuildingElementOpaque")
    }

    fn is_external_door(&self) -> Option<bool> {
        self.0.get("is_external_door").and_then(|v| v.as_bool())
    }

    fn remove_thermal_resistance_construction(&mut self) {
        self.0.shift_remove("thermal_resistance_construction");
    }
}

pub(super) fn set_control_min_name_for_heat_source(
    heat_source: &mut JsonValue,
    control_name: &str,
) -> JsonAccessResult<()> {
    heat_source
        .as_object_mut()
        .ok_or(json_error("Heat source is not an object"))?
        .insert("Controlmin".into(), json!(control_name));
    Ok(())
}

pub(super) fn set_control_max_name_for_heat_source(
    heat_source: &mut JsonValue,
    control_name: &str,
) -> JsonAccessResult<()> {
    heat_source
        .as_object_mut()
        .ok_or(json_error("Heat source is not an object"))?
        .insert("Controlmax".into(), json!(control_name));
    Ok(())
}

pub(crate) type MaybeShowerFlowRateFields = (Option<f64>, Option<bool>);

pub trait HotWaterSourceDetailsForProcessing {
    fn all_heat_sources_mut(&mut self) -> JsonAccessResult<Vec<&mut JsonValue>>;
    fn is_storage_tank(&self) -> bool;
    fn is_combi_boiler(&self) -> bool;
    fn is_hiu(&self) -> bool;
    fn is_point_of_use(&self) -> bool;
    fn is_smart_hot_water_tank(&self) -> bool;
    fn set_temp_setpnt_max(&mut self, temp_setpoint_max_name: &str);
}

pub struct HotWaterSourceDetailsJsonMap<'a>(pub &'a mut Map<std::string::String, JsonValue>);

impl HotWaterSourceDetailsForProcessing for HotWaterSourceDetailsJsonMap<'_> {
    fn all_heat_sources_mut(&mut self) -> JsonAccessResult<Vec<&mut JsonValue>> {
        let heat_sources = self
            .0
            .get_mut("HeatSource")
            .ok_or(json_error("HeatSource field not found"))?
            .as_object_mut()
            .ok_or(json_error("HeatSource field is not an object"))?;

        Ok(heat_sources.values_mut().collect_vec())
    }

    fn is_storage_tank(&self) -> bool {
        self.0
            .get("type")
            .and_then(|source_type| source_type.as_str())
            .is_some_and(|source_type| source_type == "StorageTank")
    }

    fn is_combi_boiler(&self) -> bool {
        self.0
            .get("type")
            .and_then(|source_type| source_type.as_str())
            .is_some_and(|source_type| source_type == "CombiBoiler")
    }

    fn is_hiu(&self) -> bool {
        self.0
            .get("type")
            .and_then(|source_type| source_type.as_str())
            .is_some_and(|source_type| source_type == "HIU")
    }

    fn is_point_of_use(&self) -> bool {
        self.0
            .get("type")
            .and_then(|source_type| source_type.as_str())
            .is_some_and(|source_type| source_type == "PointOfUse")
    }

    fn is_smart_hot_water_tank(&self) -> bool {
        self.0
            .get("type")
            .and_then(|source_type| source_type.as_str())
            .is_some_and(|source_type| source_type == "SmartHotWaterTank")
    }

    fn set_temp_setpnt_max(&mut self, temp_setpoint_max_name: &str) {
        self.0
            .insert("temp_setpnt_max".into(), json!(temp_setpoint_max_name));
    }
}

// The purpose of this struct is to allow deserialisation of an input just containing the data needed for the
// calc_htc_hlp function in the corpus module, so that we can ignore other areas of the data that may not be in the
// expected shape for a core input.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, Validate)]
#[serde(rename_all = "PascalCase")]
pub struct ReducedInputForCalcHtcHlp {
    #[serde(rename = "temp_internal_air_static_calcs")]
    pub(crate) temp_internal_air_static_calcs: f64,
    pub(crate) simulation_time: SimulationTime,
    pub(crate) external_conditions: Arc<ExternalConditionsInput>,
    pub(crate) energy_supply: EnergySupplyInput,
    pub(crate) control: Control,
    pub(crate) zone: ZoneDictionary,
    pub(crate) infiltration_ventilation: InfiltrationVentilation,
}

impl InputForCalcHtcHlp for ReducedInputForCalcHtcHlp {
    fn simulation_time(&self) -> &SimulationTime {
        &self.simulation_time
    }

    fn energy_supply(&self) -> &EnergySupplyInput {
        &self.energy_supply
    }

    fn external_conditions(&self) -> &ExternalConditionsInput {
        self.external_conditions.as_ref()
    }

    fn control(&self) -> &Control {
        &self.control
    }

    fn infiltration_ventilation(&self) -> &InfiltrationVentilation {
        &self.infiltration_ventilation
    }

    fn zone(&self) -> &ZoneDictionary {
        &self.zone
    }

    fn temp_internal_air_static_calcs(&self) -> f64 {
        self.temp_internal_air_static_calcs
    }
}

#[derive(Debug, Error)]
#[error("Error accessing JSON during FHS preprocessing: {0}")]
pub struct JsonAccessError(smartstring::alias::String);

pub fn json_error<T: Into<smartstring::alias::String>>(message: T) -> JsonAccessError {
    JsonAccessError(message.into())
}

pub type JsonAccessResult<T> = Result<T, JsonAccessError>;

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;
    use rstest::{fixture, rstest};
    use std::fs::File;
    use walkdir::{DirEntry, WalkDir};

    fn files_with_root(root: &str) -> Vec<DirEntry> {
        WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                !e.file_type().is_dir()
                    && e.file_name().to_str().unwrap().ends_with("json")
                    && !e
                        .path()
                        .parent()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .ends_with("results") // don't test against files in results output directories
            })
            .collect_vec()
    }

    #[fixture]
    fn fhs_files() -> Vec<DirEntry> {
        files_with_root("./examples/input/wrappers/future_homes_standard")
    }

    #[rstest]
    fn should_successfully_parse_all_fhs_demo_files(fhs_files: Vec<DirEntry>) {
        for entry in fhs_files {
            let parsed = ingest_for_processing(File::open(entry.path()).unwrap());
            assert!(
                parsed.is_ok(),
                "error was {:?} when parsing file {}",
                parsed.err().unwrap(),
                entry.file_name().to_str().unwrap()
            );
        }
    }
}

#[cfg(test)]
mod accessors_tests {
    use super::*;
    use rstest::*;

    #[fixture]
    fn events_input() -> InputForProcessing {
        let events_input_json = json!({
            "Events": {
                "Shower": {
                  "IES": [
                    {
                      "start": 4.1,
                      "duration": 6,
                      "temperature": 41.0
                    },
                    {
                      "start": 4.5,
                      "duration": 6,
                      "temperature": 41.0
                    },
                    {
                      "start": 6,
                      "duration": 6,
                      "temperature": 41.0
                    }
                  ],
                  "mixer": [
                    {
                      "start": 7,
                      "duration": 6,
                      "temperature": 41.0
                    }
                  ]
                }
          }
        });

        InputForProcessing {
            input: events_input_json,
        }
    }

    #[rstest]
    fn test_water_heating_event_by_type_and_name_when_exists(events_input: InputForProcessing) {
        assert_eq!(
            events_input
                .water_heating_event_by_type_and_name("Shower", "mixer")
                .unwrap(),
            Some(vec![WaterHeatingEvent {
                start: 7.,
                duration: Some(6.),
                volume: None,
                temperature: 41.0
            }])
        );
    }

    #[fixture]
    fn hot_water_cylinder_input() -> InputForProcessing {
        let hot_water_source_json = json!({
            "HotWaterSource": {
                "hw cylinder": {
                  "type": "StorageTank",
                  "volume": 80.0,
                  "daily_losses": 1.68,
                  "min_temp": 52.0,
                  "setpoint_temp": 55.0,
                  "ColdWaterSource": "mains water",
                  "HeatSource": {
                    "hp": {
                      "type": "HeatSourceWet",
                      "name": "hp",
                      "temp_flow_limit_upper": 65,
                      "ColdWaterSource": "mains water",
                      "EnergySupply": "mains elec",
                      "Control": "hw timer",
                      "heater_position": 0.1,
                      "thermostat_position": 0.33
                    }
                  },
                  "primary_pipework": [
                    {
                      "location": "external",
                      "internal_diameter_mm": 26.,
                      "external_diameter_mm": 28.,
                      "length": 3.0,
                      "insulation_thermal_conductivity": 0.037,
                      "insulation_thickness_mm": 25.,
                      "surface_reflectivity": false,
                      "pipe_contents": "water"
                    }
                  ]
                }
              }
        });

        InputForProcessing {
            input: hot_water_source_json,
        }
    }

    #[rstest]
    fn test_hot_water_cylinder_volume(hot_water_cylinder_input: InputForProcessing) {
        assert_eq!(
            hot_water_cylinder_input
                .hot_water_cylinder_volume()
                .unwrap(),
            Some(80.0)
        )
    }

    #[rstest]
    fn test_set_gains_for_field() {
        let mut input = InputForProcessing {
            input: json!({
                "ApplianceGains": {
                    "Clothes_washing": 2,
                }
            }),
        };

        let expected_appliance_gains = json!({
            "Clothes_washing": 2,
            "Clothes_drying": 42,
        });

        input
            .set_gains_for_field("Clothes_drying", json!(42))
            .unwrap();

        assert_eq!(
            json!(input.root_object("ApplianceGains").unwrap()),
            expected_appliance_gains
        );
    }

    #[rstest]
    fn test_water_heating_events_of_types(events_input: InputForProcessing) {
        let actual = events_input
            .water_heating_events_of_types(&["Shower"])
            .unwrap();
        let expected = vec![
            json!({
              "start": 4.1,
              "duration": 6,
              "temperature": 41.0
            }),
            json!({
              "start": 4.5,
              "duration": 6,
              "temperature": 41.0
            }),
            json!({
              "start": 6,
              "duration": 6,
              "temperature": 41.0
            }),
            json!({
              "start": 7,
              "duration": 6,
              "temperature": 41.0
            }),
        ];
        assert_eq!(actual, expected);
    }

    #[rstest]
    fn test_reset_internal_gains() {
        let base_input = json!({
            "InternalGains": {
                "metabolic gains": {
                    "start_day": 0,
                    "time_series_step": 1,
                    "schedule": {
                        "main": [1305.6, 1876.8, 2978.4, 2121.6, 3631.2, 2284.8, 4161.6, 3304.8]
                    }
                }
            }
        });
        let mut input = InputForProcessing { input: base_input };
        input.reset_internal_gains().unwrap();
        assert_eq!(input.input, json!({"InternalGains": {}}));
    }

    #[rstest]
    fn test_remove_custom_energy_supplies() {
        let base_input = json!({
            "EnergySupply": {
                "mains elec": {
                    "fuel": "electricity",
                    "ElectricBattery": {
                        "capacity": 2,
                        "charge_discharge_efficiency_round_trip": 0.8,
                        "minimum_charge_rate_one_way_trip": 0.001,
                        "maximum_charge_rate_one_way_trip": 1.5,
                        "maximum_discharge_rate_one_way_trip": 1.25,
                        "battery_location": "inside"
                    },
                    "diverter": {
                        "HeatSource": "immersion"
                    }
                },
                "mains gas": {
                    "fuel": "mains_gas"
                },
                "custom": {
                    "fuel": "custom"
                }
            }
        });
        let mut input = InputForProcessing { input: base_input };
        input.remove_custom_energy_supplies().unwrap();
        assert_eq!(
            input.input["EnergySupply"]
                .as_object()
                .unwrap()
                .keys()
                .collect_vec(),
            vec!["mains elec", "mains gas"]
        )
    }
}
