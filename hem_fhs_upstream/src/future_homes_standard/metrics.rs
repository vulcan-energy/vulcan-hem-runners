use crate::future_homes_standard::project_lookups::FuelOutput;
use serde::Serialize;
use std::sync::Arc;
use strum_macros::Display;

#[derive(Display, Copy, Clone, Debug, PartialEq, Serialize)]
enum Grade {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
}
const EER_GRADE_CUTOFFS: [(Grade, f64); 7] = [
    (Grade::G, 20.),
    (Grade::F, 38.),
    (Grade::E, 54.),
    (Grade::D, 68.),
    (Grade::C, 80.),
    (Grade::B, 91.),
    (Grade::A, f64::INFINITY),
];

const ENERGY_COST_DEFLATOR: f64 = 0.36;
const PENCE_PER_POUND: f64 = 100.;

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Metric {
    description: Arc<str>,
    grade: Option<Grade>,
    units: Arc<str>,
    value: Option<f64>,
}

#[derive(Serialize)]
pub(crate) struct Metrics {
    pub(crate) eer: Metric,
}

pub(crate) fn energy_efficiency_rating(total_floor_area: f64, by_fuel: &[FuelOutput]) -> Metric {
    // Do not provide EER if any fuel does not have unit or standing charge implemented
    let Some(costs) = by_fuel
        .iter()
        .map(
            |fuel_output| match (fuel_output.standing_charge, fuel_output.unit_price) {
                (Some(standing_charge), Some(unit_price)) => Some(
                    standing_charge as f64 + unit_price / PENCE_PER_POUND * fuel_output.eer_energy,
                ),
                _ => None,
            },
        )
        .collect::<Option<Vec<f64>>>()
    else {
        return Metric {
            description: "Energy efficiency rating (EER) not applicable for custom fuels".into(),
            units: "Normalised cost, typically in range 0-100".into(),
            value: None,
            grade: None,
        };
    };

    let total_cost = costs.iter().sum::<f64>();
    let energy_cost_factor = ENERGY_COST_DEFLATOR * total_cost / (total_floor_area + 45.);
    let eer = if energy_cost_factor >= 3.5 {
        108.8 - 120.5 * energy_cost_factor.log10()
    } else {
        100. - 16.21 * energy_cost_factor
    };

    Metric {
        description: "Energy efficiency rating based on total energy costs for space/water heating, ventilation and lighting".into(),
        units: "Normalised cost, typically in range 0-100".into(),
        value: Some(eer),
        grade: Some(energy_efficiency_grade(eer)),
    }
}

fn energy_efficiency_grade(eer: f64) -> Grade {
    EER_GRADE_CUTOFFS
        .iter()
        .find_map(|(grade, cutoff)| (eer <= *cutoff).then_some(*grade))
        .unwrap_or(Grade::A)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::future_homes_standard::project_lookups::FuelOutput;
    use home_energy_model::input::FuelType;

    #[test]
    fn test_energy_efficiency_rating() {
        // Given EER applicable energy, disaggregated by fuel
        let by_fuel = [
            FuelOutput {
                fuel: FuelType::Electricity,
                eer_energy: 1000.,
                unit_price: Some(16.49),
                standing_charge: Some(0),
            },
            FuelOutput {
                fuel: FuelType::Custom,
                eer_energy: 100.,
                unit_price: Some(1.),
                standing_charge: Some(100),
            },
        ];

        // When the EER metric is calculated
        let eer = energy_efficiency_rating(100., &by_fuel);
        let expected = Metric {
            description: "Energy efficiency rating based on total energy costs for space/water heating, ventilation and lighting".into(),
            units: "Normalised cost, typically in range 0-100".into(),
            value: Some(89.29871696551724),
            grade: Some(Grade::B),
        };

        // Then the metric shows the EER rating and grade of the dwelling
        // cost = 0 + 16.49 / 100 * 1000 + 100 + 1 / 100 * 100 = 265.9
        // ecf = 0.36 * 265.9 / (100 + 45) = 0.660165517
        // eer = 100 - 16.21 * 0.660165517 = 89.298716969
        assert_eq!(eer, expected);
    }

    #[test]
    fn test_energy_efficiency_rating_with_custom_fuel() {
        // Given EER applicable energy, disaggregated by fuel, one of which is "custom"
        let by_fuel = [
            FuelOutput {
                fuel: FuelType::Electricity,
                eer_energy: 1000.,
                unit_price: Some(16.49),
                standing_charge: Some(0),
            },
            FuelOutput {
                fuel: FuelType::Custom,
                eer_energy: 100.,
                unit_price: None,
                standing_charge: None,
            },
        ];

        // When the EER metric is calculated
        let eer = energy_efficiency_rating(100., &by_fuel);
        let expected = Metric {
            description: "Energy efficiency rating (EER) not applicable for custom fuels".into(),
            units: "Normalised cost, typically in range 0-100".into(),
            value: None,
            grade: None,
        };

        // Then the metric shows the EER as unimplemented
        assert_eq!(eer, expected);
    }
}
