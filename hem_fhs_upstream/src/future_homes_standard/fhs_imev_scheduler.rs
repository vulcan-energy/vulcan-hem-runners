use crate::future_homes_standard::input::InputForProcessing;
use anyhow::{anyhow, bail};
use home_energy_model::core::units::MINUTES_PER_HOUR;
use indexmap::IndexMap;
use itertools::Itertools;
use serde_json::{json, Map, Value};

const SHOWER_RUN_ON: f64 = 0.25; // hours
const BATH_MINIMUM_DURATION: f64 = 0.5; // hours
const OTHER_RUN_ON: f64 = 1. / 12.; // hours

#[derive(Copy, Clone, Debug, PartialEq)]
enum EventType {
    Tapping,
    Cooking,
}

#[derive(Copy, Clone, Debug)]
struct Event {
    start: f64,
    duration: f64,
    event_type: Option<EventType>,
}

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
struct Time {
    start: f64,
    end: f64,
}

type TimeChunk = Vec<Time>;

impl Event {
    fn chunkify(
        &self,
        simulation_start_time: i32,
        simulation_end_time: i32,
    ) -> anyhow::Result<TimeChunk> {
        // if duration takes us beyond the simulation time or the start is < simulation start time
        // break into multiple events strictly starting and ending during the simulation
        // with start < end
        let simulation_start_time = simulation_start_time as f64;
        let simulation_end_time = simulation_end_time as f64;
        if self.start + self.duration <= simulation_start_time || self.start >= simulation_end_time
        {
            bail!(
                "Event (start={}, duration={}) wholly outside the simulation time ({} to {})",
                self.start,
                self.duration,
                simulation_start_time,
                simulation_end_time
            );
        }
        if self.duration > simulation_end_time - simulation_start_time {
            bail!(
                "Event (start={}, duration={}) is longer than the simulation time ({} to {})",
                self.start,
                self.duration,
                simulation_start_time,
                simulation_end_time
            );
        }

        let mut times = TimeChunk::new();

        if self.start < simulation_start_time {
            let underspill = simulation_start_time - self.start;
            times.push(Time {
                start: simulation_end_time - underspill,
                end: simulation_end_time,
            });
            times.push(Time {
                start: simulation_start_time,
                end: simulation_start_time + self.duration - underspill,
            })
        } else if self.start + self.duration > simulation_end_time {
            let overspill = self.start + self.duration - simulation_end_time;
            times.push(Time {
                start: self.start,
                end: self.start + self.duration - overspill,
            });
            times.push(Time {
                start: simulation_start_time,
                end: simulation_start_time + overspill,
            });
        } else {
            times.push(Time {
                start: self.start,
                end: self.start + self.duration,
            });
        };

        Ok(times)
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
struct IMev {
    name: String,
    on_times: TimeChunk,
    simulation_start_time: i32,
    simulation_end_time: i32,
    timestep: f64,
}

impl IMev {
    fn new(
        name: &str,
        simulation_start_time: i32,
        simulation_end_time: i32,
        timestep: f64,
    ) -> Self {
        Self {
            name: name.into(),
            on_times: Default::default(),
            simulation_start_time,
            simulation_end_time,
            timestep,
        }
    }

    #[cfg(test)]
    fn set_on_times(&mut self, on_times: TimeChunk) {
        self.on_times = on_times;
    }

    fn assign_on_time(&mut self, event: &Event) -> anyhow::Result<()> {
        let times = event.chunkify(self.simulation_start_time, self.simulation_end_time)?;
        self.on_times.extend(times);
        Ok(())
    }

    fn on_fraction(&self, event: &Event) -> anyhow::Result<f64> {
        // fraction of the given event's duration that the iMEV is on
        let test_periods = event.chunkify(self.simulation_start_time, self.simulation_end_time)?;
        let total_duration: f64 = test_periods.iter().map(|p| p.end - p.start).sum::<f64>();
        let mut duration_on = 0.;
        for period in test_periods {
            let mut on_times = self
                .on_times
                .iter()
                .filter(|on| !(on.start > period.end || on.end < period.start))
                .cloned()
                .collect::<TimeChunk>();
            let mut current_time_end = period.start;
            while current_time_end < period.end && !on_times.is_empty() {
                let current_time = on_times
                    .iter()
                    .min_by(|a, b| a.start.partial_cmp(&b.start).unwrap())
                    .unwrap();
                duration_on +=
                    (period.end.min(current_time.end)) - (current_time_end.max(current_time.start));
                current_time_end = current_time.end;
                // Keep only on_times that extend beyond current_time_end
                on_times.retain(|on| on.end > current_time_end);
            }
        }
        Ok(duration_on / total_duration)
    }

    fn schedulise(&self) -> anyhow::Result<Vec<f64>> {
        (self.simulation_start_time
            ..(self.simulation_end_time as f64 / self.timestep).round() as i32)
            .map(|i| {
                self.on_fraction(&Event {
                    start: i as f64 * self.timestep,
                    duration: self.timestep,
                    event_type: None,
                })
            })
            .collect()
    }
}

#[derive(PartialEq, PartialOrd)]
struct IMevCycle {
    imevs: Vec<IMev>,
}

impl From<Vec<IMev>> for IMevCycle {
    fn from(imevs: Vec<IMev>) -> Self {
        IMevCycle { imevs }
    }
}

impl IMevCycle {
    fn get_next_best_imev(&mut self, event: &Event) -> anyhow::Result<Option<&mut IMev>> {
        if self.imevs.is_empty() {
            return Ok(None);
        }

        // Select an iMEV from the group using the best availability as the first criterion
        // Tie break by the least used fan to try to keep fan use relatively even
        let mut scores_and_imevs: Vec<(f64, usize, usize)> = Vec::new();
        for (i, imev) in self.imevs.iter().enumerate() {
            let availability_score = imev.on_fraction(event)?;
            let times_on_score = imev.on_times.len();
            scores_and_imevs.push((availability_score, times_on_score, i));
        }

        Ok(scores_and_imevs
            .into_iter()
            .min_by(|a, b| {
                (a.0, a.1)
                    .partial_cmp(&(b.0, b.1))
                    .expect("Unable to compare scores as NaN encountered")
            })
            .and_then(|(_, _, imev)| self.imevs.get_mut(imev)))
    }
}

/// Turn on intermittent MEVs whenever cooking or tapping events occur.
/// Largest intermittent MEV used for cooking events, all other MEVs used for tapping events.
/// Mutates the proj_dict. Does nothing if there are no intermittent MEVs in the proj_dict.
/// Provide start, end and step in units of hours.
pub(crate) fn create_imev_pattern(
    input: &mut InputForProcessing,
    start: f64,
    end: f64,
    step: f64,
) -> anyhow::Result<()> {
    let (mut kitchen_imev, mut non_kitchen_imevs) = {
        let intermittent_mevs: IndexMap<String, &Value> = input.input["InfiltrationVentilation"]
            ["MechanicalVentilation"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Missing MechanicalVentilation section in input"))?
            .iter()
            .filter(|(_, vent)| vent["vent_type"].as_str() == Some("Intermittent MEV"))
            .map(|(vent_name, vent)| (vent_name.to_string(), vent))
            .collect();
        if intermittent_mevs.is_empty() {
            return Ok(());
        }
        let largest_vent_name = intermittent_mevs
            .iter()
            .max_by(|(a_name, a_vent), (b_name, b_vent)| {
                let a_flow_rate = a_vent
                    .get("design_outdoor_air_flow_rate")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.);
                let b_flow_rate = b_vent
                    .get("design_outdoor_air_flow_rate")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.);
                (a_flow_rate, a_name)
                    .partial_cmp(&(b_flow_rate, b_name))
                    .expect("Unable to compare design outdoor air flow rates as NaN encountered")
            })
            .expect("No intermittent ventilation MEVs found in input")
            .0;
        let kitchen_imev: IMevCycle =
            vec![IMev::new(largest_vent_name, start as i32, end as i32, step)].into();
        let non_kitchen_imevs: IMevCycle = intermittent_mevs
            .keys()
            .filter(|&vent_name| vent_name != largest_vent_name)
            .map(|vent_name| IMev::new(vent_name, start as i32, end as i32, step))
            .collect::<Vec<IMev>>()
            .into();

        (kitchen_imev, non_kitchen_imevs)
    };

    // mega list of all events. We can sort them into start time order to avoid prejudicing
    // the fans more towards certain events where demand is high
    // Tapping events from proj_dict["Events"] have a "start" in hours, but
    // a "duration" in minutes, so the latter has to be converted to hours

    let events = input.input["Events"]
        .as_object()
        .expect("Event object is always expected to be present");
    let cook_enduses = ["Oven", "Hobs"];

    let mut events_list: Vec<Event> = {
        // get the event count so we know capacity needed for the vec we're pushing into
        let mut event_count = ["Shower", "Bath", "Other"]
            .into_iter()
            .map(|node| {
                events
                    .get(node)
                    .and_then(|shower| shower.as_array())
                    .map(|events| events.len())
                    .unwrap_or(0)
            })
            .sum();
        let appliance_gains = appliance_gains_from_input(&input.input)?;
        event_count += cook_enduses
            .iter()
            .map(|enduse| {
                appliance_gains
                    .get(*enduse)
                    .and_then(|enduse_gain| enduse_gain.as_array())
                    .map(|enduse_gains| enduse_gains.len())
                    .unwrap_or(0)
            })
            .sum::<usize>();
        Vec::with_capacity(event_count)
    };

    if let Some(showers) = events
        .get("Shower")
        .and_then(|s| s.as_object())
        .map(|v| v.values())
    {
        for shower in showers {
            for event in shower.as_array().into_iter().flatten() {
                events_list.push(Event {
                    start: event["start"].as_f64().ok_or_else(|| {
                        anyhow!("Shower event start expected to be available as number")
                    })?,
                    duration: event["duration"].as_f64().ok_or_else(|| {
                        anyhow!("Shower event duration expected to be available as number")
                    })? / MINUTES_PER_HOUR as f64
                        + SHOWER_RUN_ON,
                    event_type: EventType::Tapping.into(),
                });
            }
        }
    }
    if let Some(baths) = events
        .get("Bath")
        .and_then(|s| s.as_object())
        .map(|v| v.values())
    {
        for bath in baths {
            for event in bath.as_array().into_iter().flatten() {
                events_list.push(Event {
                    start: event["start"].as_f64().ok_or_else(|| {
                        anyhow!("Bath event start expected to be available as number")
                    })?,
                    duration: BATH_MINIMUM_DURATION.max(
                        event["duration"].as_f64().ok_or_else(|| {
                            anyhow!("Bath event duration expected to be available as number")
                        })? / MINUTES_PER_HOUR as f64,
                    ),
                    event_type: EventType::Tapping.into(),
                })
            }
        }
    }
    if let Some(others) = events
        .get("Other")
        .and_then(|s| s.as_object())
        .map(|v| v.values())
    {
        for other in others {
            for event in other.as_array().into_iter().flatten() {
                events_list.push(Event {
                    start: event["start"].as_f64().ok_or_else(|| {
                        anyhow!("Other water use event start expected to be available as number")
                    })?,
                    duration: event["duration"].as_f64().ok_or_else(|| {
                        anyhow!("Other water use event duration expected to be available as number")
                    })? / MINUTES_PER_HOUR as f64
                        + OTHER_RUN_ON,
                    event_type: EventType::Tapping.into(),
                })
            }
        }
    }

    // Cooking events from proj_dict["ApplianceGains"][cook_enduse]["Events"]
    // have "start" and "duration" in hours so no conversion is needed
    {
        let appliance_gains = appliance_gains_from_input(&input.input)?;
        for cook_enduse in cook_enduses {
            if let Some(cook_events) = appliance_gains
                .get(cook_enduse)
                .and_then(|v| v.get("Events"))
                .and_then(|v| v.as_array())
            {
                for event in cook_events {
                    events_list.push(Event {
                        start: event["start"].as_f64().ok_or_else(|| {
                            anyhow!("Event start expected to be available as number")
                        })?,
                        duration: event["duration"].as_f64().ok_or_else(|| {
                            anyhow!("Event duration expected to be available as number")
                        })?,
                        event_type: EventType::Cooking.into(),
                    })
                }
            }
        }
    }

    for event in events_list.iter().sorted_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .expect("Unable to compare event start times as NaN encountered")
    }) {
        if event
            .event_type
            .is_some_and(|event_type| event_type == EventType::Cooking)
            || non_kitchen_imevs.imevs.is_empty()
        {
            kitchen_imev.imevs[0].assign_on_time(event)?;
        } else if event
            .event_type
            .is_some_and(|event_type| event_type == EventType::Tapping)
        {
            let best_non_kitchen_imev = non_kitchen_imevs
                .get_next_best_imev(event)?
                .ok_or_else(|| anyhow!("At least one non-kitchen IMEV is expected"))?;
            best_non_kitchen_imev.assign_on_time(event)?;
        }
    }

    for vent in kitchen_imev
        .imevs
        .iter()
        .chain(non_kitchen_imevs.imevs.iter())
    {
        let controlname = format!("_intermittent_MEV_control: {}", vent.name);
        input.set_control_for_mechanical_ventilation(&vent.name, &controlname)?;
        input.add_control(
            &controlname,
            json!({
                "type": "SetpointTimeControl",
                "start_day": 0,
                "time_series_step": step,
                "schedule": {
                    "main": vent.schedulise()?
                }
            }),
        )?;
    }

    Ok(())
}

fn appliance_gains_from_input(input: &Value) -> anyhow::Result<&Map<String, Value>> {
    input
        .get("ApplianceGains")
        .ok_or_else(|| anyhow!("ApplianceGains missing"))?
        .as_object()
        .ok_or_else(|| anyhow!("ApplianceGains is not an object"))
}

#[cfg(test)]
mod test {
    mod test_event {
        use crate::future_homes_standard::fhs_imev_scheduler::{Event, Time};

        #[test]
        fn test_event_chunkifies_start_time_less_than_start_time() {
            // Given an event that starts before t=0
            let event = Event {
                start: -1.,
                duration: 5.,
                event_type: None,
            };
            // When the event is chunkified
            let times = event.chunkify(0, 10).unwrap();
            // Then the event is split into two time periods
            assert_eq!(
                times[0],
                Time {
                    start: 9.,
                    end: 10.
                }
            );
            assert_eq!(times[1], Time { start: 0., end: 4. });
        }

        #[test]
        fn test_event_chunkifies_duration_overspilling_loop_time() {
            // Given an event that ends after the end of the time period
            let event = Event {
                start: 8.,
                duration: 5.,
                event_type: None,
            };
            // When the event is chunkified
            let times = event.chunkify(0, 10).unwrap();
            // Then the event is split into two time periods
            assert_eq!(
                times[0],
                Time {
                    start: 8.,
                    end: 10.
                }
            );
            assert_eq!(times[1], Time { start: 0., end: 3. });
        }

        #[test]
        fn test_event_chunkifies_covering_both_ends_of_time_window() {
            // Given an event that ends after the end of the time period
            let event = Event {
                start: -1.,
                duration: 12.,
                event_type: None,
            };
            // The event can't be chunkified
            let times = event.chunkify(0, 10);
            assert!(times.is_err());
            // So appropriate error
            let errror = times.unwrap_err().to_string();
            assert_eq!(
                errror,
                "Event (start=-1, duration=12) is longer than the simulation time (0 to 10)"
            );
        }

        #[test]
        fn test_event_falling_within_loop_time() {
            // Given an event that ends after the end of the time period
            let event = Event {
                start: 1.,
                duration: 5.,
                event_type: None,
            };
            // When the event is chunkified
            let times = event.chunkify(0, 10).unwrap();
            // Then the event is converted to a single time
            assert_eq!(times[0], Time { start: 1., end: 6. });
        }

        #[test]
        fn test_error_for_event_before_time_window() {
            // Given an event that ends before the start of the time period
            let event = Event {
                start: -10.,
                duration: 5.,
                event_type: None,
            };
            let times = event.chunkify(0, 10);
            // The event can't be chunkified
            assert!(times.is_err());
            // So appropriate error
            assert_eq!(
                times.unwrap_err().to_string(),
                "Event (start=-10, duration=5) wholly outside the simulation time (0 to 10)"
            );
        }

        #[test]
        fn test_error_for_event_after_time_window() {
            // Given an event that ends before the start of the time period
            let event = Event {
                start: 15.,
                duration: 5.,
                event_type: None,
            };
            let times = event.chunkify(0, 10);
            // The event can't be chunkified
            assert!(times.is_err());
            // So appropriate error
            assert_eq!(
                times.unwrap_err().to_string(),
                "Event (start=15, duration=5) wholly outside the simulation time (0 to 10)"
            );
        }

        #[test]
        fn test_event_chunkifies_start_time_less_than_start_time_non_zero_start_time() {
            // Given an event that starts before t=0
            let event = Event {
                start: 0.,
                duration: 5.,
                event_type: None,
            };
            // When the event is chunkified
            let times = event.chunkify(1, 11).unwrap();
            // Then the event is split into two time periods
            assert_eq!(
                times[0],
                Time {
                    start: 10.,
                    end: 11.
                }
            );
            assert_eq!(times[1], Time { start: 1., end: 5. });
        }
    }

    mod test_imev_schedulise {
        use crate::future_homes_standard::fhs_imev_scheduler::{IMev, Time};
        use approx::assert_relative_eq;

        #[test]
        fn test_returns_on_fractions_for_non_overlapping_on_times() {
            // Given an IMEV with on_times within a given time window
            let mut imev = IMev::new("venty mcventface", 2, 5, 1.);
            imev.set_on_times(
                [
                    Time { start: 2., end: 3. },
                    Time {
                        start: 3.5,
                        end: 4.5,
                    },
                ]
                .into(),
            );
            // When converting to a schedule
            let schedule = imev.schedulise().unwrap();
            // Then the schedule reflects the time that the vent is on in each timestep
            assert_eq!(schedule, vec![1., 0.5, 0.5]);
        }

        #[test]
        fn test_returns_on_fractions_for_overlapping_on_times() {
            // Given an IMEV with on_times within a given time window
            let mut imev = IMev::new("venty mcventface", 2, 5, 1.);
            imev.set_on_times(
                [
                    Time { start: 2., end: 3. },
                    Time {
                        start: 2.5,
                        end: 3.5,
                    },
                ]
                .into(),
            );
            // When converting to a schedule
            let schedule = imev.schedulise().unwrap();
            // Then the schedule reflects the time that the vent is on in each timestep
            assert_eq!(schedule, vec![1., 0.5, 0.]);
        }

        #[test]
        fn test_returns_on_fractions_for_overspilling_on_times() {
            // Given an IMEV with on_times within a given time window
            let mut imev = IMev::new("venty mcventface", 2, 5, 1.);
            imev.set_on_times(
                [
                    Time {
                        start: 2.,
                        end: 2.2,
                    },
                    Time {
                        start: 4.2,
                        end: 5.,
                    },
                ]
                .into(),
            );
            // When converting to a schedule
            let schedule = imev.schedulise().unwrap();
            // Then the schedule reflects the time that the vent is on in each timestep
            assert_relative_eq!(schedule[0], 0.2);
            assert_relative_eq!(schedule[1], 0.);
            assert_relative_eq!(schedule[2], 0.8);
        }
    }

    mod test_create_imev_pattern {
        use crate::future_homes_standard::fhs_imev_scheduler::create_imev_pattern;
        use crate::future_homes_standard::input::InputForProcessing;
        use approx::assert_relative_eq;
        use rstest::{fixture, rstest};
        use serde_json::{json, Value};

        #[fixture]
        fn ventilation_input() -> InputForProcessing {
            // Tapping event durations are in minutes, cooking event durations in hours
            let ventilation_input_json = json!({
                "ApplianceGains": {"Oven": {"Events": [{"start": 0, "duration": 2}]}},
                "Control": {},
                "Events": {
                    "Shower": {"shower_1": [{"start": 2, "duration": 6}]},
                    "Bath": {},
                    "Other": {},
                },
                "InfiltrationVentilation": {
                    "MechanicalVentilation": {
                        "big_vent": {
                            "design_outdoor_air_flow_rate": 400,
                            "vent_type": "Intermittent MEV",
                        },
                        "little_vent": {
                            "design_outdoor_air_flow_rate": 50,
                            "vent_type": "Intermittent MEV",
                        },
                    }
                },
            });

            InputForProcessing {
                input: ventilation_input_json,
            }
        }

        #[rstest]
        fn test_simple_set_of_fulfillable_events(mut ventilation_input: InputForProcessing) {
            // Given:
            //   a valid single valid mechanical vent each of >216 m3/hr and < 216 m3/hr
            //   a couple of shower events that don't overlap (including run-on)
            //   a couple of cooking events that don't overlap

            // When the scheduler is run for a 3 hour period in steps of 0.5 hours
            create_imev_pattern(&mut ventilation_input, 0., 3., 0.5).unwrap();
            // Then the project dictionary is mutated with a Control for each vent
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("big_vent")
                    .unwrap(),
                "_intermittent_MEV_control: big_vent"
            );
            assert_eq!(
                ventilation_input.input["Control"]["_intermittent_MEV_control: big_vent"]
                    ["schedule"]["main"],
                json!([1.0, 1.0, 1.0, 1.0, 0.0, 0.0])
            );
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("little_vent")
                    .unwrap(),
                "_intermittent_MEV_control: little_vent"
            );
            let expected_little_vent_schedule = [0.0, 0.0, 0.0, 0.0, 0.7, 0.0];
            for (i, vent_schedule_entry) in ventilation_input.input["Control"]
                ["_intermittent_MEV_control: little_vent"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_relative_eq!(
                    vent_schedule_entry.as_f64().unwrap(),
                    expected_little_vent_schedule[i],
                    epsilon = 1e-7
                );
            }
        }

        fn merge_json_values(a: &mut Value, b: Value) {
            match (a, b) {
                (a @ &mut Value::Object(_), Value::Object(b)) => {
                    let a = a.as_object_mut().unwrap();
                    // clobber target object if item being merged is empty object
                    if b.is_empty() {
                        a.clear();
                        return;
                    }
                    for (k, v) in b {
                        merge_json_values(a.entry(k).or_insert(Value::Null), v);
                    }
                }
                (a, b) => *a = b,
            }
        }

        #[rstest]
        fn test_tapping_does_not_use_kitchen_fan_even_when_overlap_unavoidable(
            mut ventilation_input: InputForProcessing,
        ) {
            // Given:
            //    kitchen cooking event runs 0-2h
            //    two shower events overlap each other inside the first hour
            //    tapping must remain on non-kitchen fan only, extending runtime as needed
            merge_json_values(
                &mut ventilation_input.input,
                json!({
                    "ApplianceGains": {"Oven": {"Events": [{"start": 0, "duration": 2}]}},
                    "Events": {
                        "Shower": {
                            // each shower: 6 mins = 0.1h, plus 0.25h run-on => 0.35h
                            // intervals: [2.00, 2.35] and [2.20, 2.55] => union [2.00, 2.55]
                            "shower_1": [{"start": 2.0, "duration": 6}, {"start": 2.2, "duration": 6}]
                        },
                        "Bath": {},
                        "Other": {},
                    },
                }),
            );

            // When the scheduler is run for a 3 hour period in steps of 1 hour
            create_imev_pattern(&mut ventilation_input, 0., 3., 1.).unwrap();
            // then the kitchen fan is for cooking only (0-2h)
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("big_vent")
                    .unwrap(),
                "_intermittent_MEV_control: big_vent"
            );
            assert_eq!(
                ventilation_input.input["Control"]["_intermittent_MEV_control: big_vent"]
                    ["schedule"]["main"],
                json!([1.0, 1.0, 0.0])
            );
            // and the non-kitchen fan handles both overlapping showers by extending coverage
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("little_vent")
                    .unwrap(),
                "_intermittent_MEV_control: little_vent"
            );
            let expected_little_vent_schedule = [0.0, 0.0, 0.55];
            for (i, vent_schedule_entry) in ventilation_input.input["Control"]
                ["_intermittent_MEV_control: little_vent"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_relative_eq!(
                    vent_schedule_entry.as_f64().unwrap(),
                    expected_little_vent_schedule[i],
                    epsilon = 1e-7
                );
            }
        }

        #[rstest]
        fn test_events_in_the_same_timestep_with_a_gap_between(
            mut ventilation_input: InputForProcessing,
        ) {
            // Given:
            //    a couple of shower events that don't overlap (including run-on) in the same timestep
            merge_json_values(
                &mut ventilation_input.input,
                json!({
                    "ApplianceGains": {},
                    "Events": {
                        "Shower": {
                            "shower_1": [{"start": 0, "duration": 6}, {"start": 0.4, "duration": 6}]
                        },
                        "Bath": {},
                        "Other": {},
                    },
                }),
            );

            // When the scheduler is run for a 3 hour period in steps of 1 hour
            create_imev_pattern(&mut ventilation_input, 0., 3., 1.).unwrap();
            // Then the kitchen fan should be unused (no cooking events)
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("big_vent")
                    .unwrap(),
                "_intermittent_MEV_control: big_vent"
            );
            assert_eq!(
                ventilation_input.input["Control"]["_intermittent_MEV_control: big_vent"]
                    ["schedule"]["main"],
                json!([0.0, 0.0, 0.0])
            );
            // And the shower runs for 0.1 + 0.25 run on, stops for 0.05 and then repeats
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("little_vent")
                    .unwrap(),
                "_intermittent_MEV_control: little_vent"
            );
            let expected_little_vent_schedule = [0.7, 0.0, 0.0];
            for (i, vent_schedule_entry) in ventilation_input.input["Control"]
                ["_intermittent_MEV_control: little_vent"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_relative_eq!(
                    vent_schedule_entry.as_f64().unwrap(),
                    expected_little_vent_schedule[i],
                    epsilon = 1e-7
                );
            }
        }

        #[rstest]
        fn test_extends_fan_run_time_as_best_as_possible_when_overlap_unavoidable(
            mut ventilation_input: InputForProcessing,
        ) {
            // Given:
            //    a valid mechanical vent each of >216 m3/hr and < 216 m3/hr
            //    a cooking event and two shower events that overlap
            merge_json_values(
                &mut ventilation_input.input,
                json!({
                    "ApplianceGains": {"Oven": {"Events": [{"start": 0, "duration": 2}]}},
                    "Events": {
                        "Shower": {
                            "shower_1": [{"start": 0, "duration": 6}, {"start": 0.2, "duration": 6}]
                        },
                        "Bath": {},
                        "Other": {},
                    },
                }),
            );

            // When the scheduler is run for a 3 hour period in steps of 1 hour
            create_imev_pattern(&mut ventilation_input, 0., 3., 1.).unwrap();

            // Then the project dictionary is mutated with a Control for each vent
            // The best option is for the second shower to use the smaller vent because
            // there is some availability
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("big_vent")
                    .unwrap(),
                "_intermittent_MEV_control: big_vent"
            );
            assert_eq!(
                ventilation_input.input["Control"]["_intermittent_MEV_control: big_vent"]
                    ["schedule"]["main"],
                json!([1.0, 1.0, 0.0])
            );
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("little_vent")
                    .unwrap(),
                "_intermittent_MEV_control: little_vent"
            );
            let expected_little_vent_schedule = [
                0.55, // from start of first shower until 0.25 after the end of the second
                0.0, 0.0,
            ];
            for (i, vent_schedule_entry) in ventilation_input.input["Control"]
                ["_intermittent_MEV_control: little_vent"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_relative_eq!(
                    vent_schedule_entry.as_f64().unwrap(),
                    expected_little_vent_schedule[i],
                    epsilon = 1e-7
                );
            }
        }

        #[rstest]
        fn test_events_whose_run_on_times_overflow_the_time_period(
            mut ventilation_input: InputForProcessing,
        ) {
            // Given:
            //    a valid mechanical vent each of >216 m3/hr and < 216 m3/hr
            //    a shower event within 0.25 hours of the end of the period
            //    a bath event within 0.5 hours of the start of the period
            merge_json_values(
                &mut ventilation_input.input,
                json!({
                    "ApplianceGains": {},
                    "Events": {
                        "Shower": {"shower_1": [{"start": 2.75, "duration": 30}]},
                        "Bath": {"bath_1": [{"start": 0.25, "duration": 30}]},
                        "Other": {},
                    },
                }),
            );

            // When the scheduler is run for a 3 hour period in steps of 1 hour
            create_imev_pattern(&mut ventilation_input, 0., 3., 1.).unwrap();

            // Then the project dictionary is mutated with a Control for each vent
            // The overflowing events wrap round from last timestep to first timestep
            // The bath starts first so it gets the first choice small vent
            // The shower extends the bath event due to overlap with bath
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("big_vent")
                    .unwrap(),
                "_intermittent_MEV_control: big_vent"
            );
            assert_eq!(
                ventilation_input.input["Control"]["_intermittent_MEV_control: big_vent"]
                    ["schedule"]["main"],
                json!([0.0, 0.0, 0.0])
            );
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("little_vent")
                    .unwrap(),
                "_intermittent_MEV_control: little_vent"
            );
            let expected_little_vent_schedule = [
                0.75, // Bath: [0.25, 0.75] and shower: [2.75, 3.0] plus wrap [0.0, 0.5]
                // => union: [0.0, 0.75]
                0.0, 0.25, // 0.25 of shower event
            ];
            for (i, vent_schedule_entry) in ventilation_input.input["Control"]
                ["_intermittent_MEV_control: little_vent"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_relative_eq!(
                    vent_schedule_entry.as_f64().unwrap(),
                    expected_little_vent_schedule[i],
                    epsilon = 1e-7
                );
            }
        }

        #[rstest]
        fn test_bath_event_minimum_duration(mut ventilation_input: InputForProcessing) {
            // Given:
            //    a valid mechanical vent each of >216 m3/hr and < 216 m3/hr and
            //    a bath event lasting 15mins from the start of the period
            merge_json_values(
                &mut ventilation_input.input,
                json!({
                    "ApplianceGains": {},
                    "Events": {
                        "Shower": {},
                        "Bath": {"bath_1": [{"start": 0, "duration": 15}]},
                        "Other": {},
                    },
                }),
            );

            // When the scheduler is run for an hour in an one hour step
            create_imev_pattern(&mut ventilation_input, 0., 1., 1.).unwrap();

            // Then the project dict is updated with a control for the smaller vent
            // that runs for 30mins
            assert_eq!(
                ventilation_input
                    .mechanical_ventilation_control_by_key("little_vent")
                    .unwrap(),
                "_intermittent_MEV_control: little_vent"
            );
            let expected_little_vent_schedule = [0.5]; // max(0.5, 0.25) -> (default_bath_duration, actual_bath_duration)
            for (i, vent_schedule_entry) in ventilation_input.input["Control"]
                ["_intermittent_MEV_control: little_vent"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_relative_eq!(
                    vent_schedule_entry.as_f64().unwrap(),
                    expected_little_vent_schedule[i],
                    epsilon = 1e-7
                );
            }
        }

        #[rstest]
        fn test_full_scale_example() {
            let mut project_dict = InputForProcessing {
                input: serde_json::from_str(include_str!(
                    "test_assets/fixtures/test_demo_FHS_multiple_intermittent_MEV_events.json"
                ))
                .unwrap(),
            };

            create_imev_pattern(&mut project_dict, 0., 8760., 0.5).unwrap();

            let expected = InputForProcessing {
                input: serde_json::from_str(include_str!(
                    "test_assets/expected_results/test_demo_FHS_multiple_intermittent_MEV_schedules.json"
                )).unwrap(),
            };

            let actual_schedule_1 = project_dict.input["Control"]
                ["_intermittent_MEV_control: mechvent1"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_f64)
                .collect::<Vec<_>>();
            let expected_schedule_1 = expected.input["Control"]
                ["_intermittent_MEV_control: mechvent1"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_f64)
                .collect::<Vec<_>>();
            for (i, entry) in actual_schedule_1.iter().enumerate() {
                assert_relative_eq!(entry, &expected_schedule_1[i], epsilon = 1e-7);
            }

            let actual_schedule_2 = project_dict.input["Control"]
                ["_intermittent_MEV_control: mechvent2"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_f64)
                .collect::<Vec<_>>();
            let expected_schedule_2 = expected.input["Control"]
                ["_intermittent_MEV_control: mechvent2"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_f64)
                .collect::<Vec<_>>();
            for (i, entry) in actual_schedule_2.iter().enumerate() {
                assert_relative_eq!(entry, &expected_schedule_2[i], epsilon = 1e-7);
            }

            let actual_schedule_3 = project_dict.input["Control"]
                ["_intermittent_MEV_control: mechvent3"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_f64)
                .collect::<Vec<_>>();
            let expected_schedule_3 = expected.input["Control"]
                ["_intermittent_MEV_control: mechvent3"]["schedule"]["main"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_f64)
                .collect::<Vec<_>>();
            for (i, entry) in actual_schedule_3.iter().enumerate() {
                assert_relative_eq!(entry, &expected_schedule_3[i], epsilon = 1e-7);
            }
        }
    }
}
