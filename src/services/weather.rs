//! Weather from Open-Meteo (free, no API key): current conditions, the next
//! hours and today's high/low for a city the user picks from the menu.
//! Refreshed every 20 minutes; the city is geocoded once and its
//! coordinates are kept in the settings.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};

use super::{EventBus, ServiceEvent};

const REFRESH: Duration = Duration::from_secs(20 * 60);
const RETRY: Duration = Duration::from_secs(3 * 60);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HourForecast {
    /// Local hour of day (0..24).
    pub hour: u32,
    pub temperature: f32,
    pub code: u8,
    pub is_day: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub location: String,
    pub temperature: f32,
    pub feels_like: f32,
    pub code: u8,
    pub is_day: bool,
    pub humidity: f32,
    pub wind: f32,
    pub high: f32,
    pub low: f32,
    pub hours: Vec<HourForecast>,
}

#[derive(Debug)]
pub enum WeatherEvent {
    Updated(Weather),
    /// Result of a city lookup started from the menu.
    Located(Result<Location, String>),
    Failed(String),
}

pub struct WeatherHandle {
    location: watch::Sender<Option<Location>>,
    lookup: mpsc::UnboundedSender<String>,
}

impl WeatherHandle {
    pub fn set_location(&self, location: Option<Location>) {
        self.location.send_if_modified(|current| {
            let changed = *current != location;
            *current = location;
            changed
        });
    }

    /// Geocodes `city`; the answer arrives as [`WeatherEvent::Located`].
    pub fn lookup(&self, city: String) {
        let _ = self.lookup.send(city);
    }
}

#[derive(Deserialize)]
struct GeoResponse {
    #[serde(default)]
    results: Vec<GeoResult>,
}

#[derive(Deserialize)]
struct GeoResult {
    name: String,
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    country: Option<String>,
}

#[derive(Deserialize)]
struct Forecast {
    current: Current,
    hourly: Hourly,
    daily: Daily,
}

#[derive(Deserialize)]
struct Current {
    time: String,
    temperature_2m: f32,
    apparent_temperature: f32,
    relative_humidity_2m: f32,
    wind_speed_10m: f32,
    weather_code: u8,
    is_day: u8,
}

#[derive(Deserialize)]
struct Hourly {
    time: Vec<String>,
    temperature_2m: Vec<f32>,
    weather_code: Vec<u8>,
    is_day: Vec<u8>,
}

#[derive(Deserialize)]
struct Daily {
    temperature_2m_max: Vec<f32>,
    temperature_2m_min: Vec<f32>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("WindowsDynamicNotch/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default()
}

async fn geocode(client: &reqwest::Client, city: &str) -> Result<Location, String> {
    let response: GeoResponse = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[("name", city), ("count", "1"), ("language", "en"), ("format", "json")])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let hit = response.results.into_iter().next().ok_or_else(|| format!("no place named \"{city}\""))?;
    let name = match hit.country {
        Some(country) => format!("{}, {country}", hit.name),
        None => hit.name,
    };
    Ok(Location { name, latitude: hit.latitude, longitude: hit.longitude })
}

/// "2026-10-24T14:00" -> 14
fn hour_of(time: &str) -> Option<u32> {
    time.get(11..13)?.parse().ok()
}

async fn fetch(client: &reqwest::Client, location: &Location) -> Result<Weather, String> {
    let forecast: Forecast = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", location.latitude.to_string()),
            ("longitude", location.longitude.to_string()),
            (
                "current",
                "temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code,is_day".into(),
            ),
            ("hourly", "temperature_2m,weather_code,is_day".into()),
            ("daily", "temperature_2m_max,temperature_2m_min".into()),
            ("timezone", "auto".into()),
            ("forecast_days", "2".into()),
        ])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let current = &forecast.current;
    // Hourly entries are local times; start after the current hour.
    let now_prefix = current.time.get(..13).unwrap_or_default();
    let start = forecast.hourly.time.iter().position(|t| t.as_str() > now_prefix).unwrap_or(0);
    let hours = (start..forecast.hourly.time.len())
        .take(6)
        .filter_map(|i| {
            Some(HourForecast {
                hour: hour_of(&forecast.hourly.time[i])?,
                temperature: *forecast.hourly.temperature_2m.get(i)?,
                code: *forecast.hourly.weather_code.get(i)?,
                is_day: *forecast.hourly.is_day.get(i)? != 0,
            })
        })
        .collect();
    Ok(Weather {
        location: location.name.clone(),
        temperature: current.temperature_2m,
        feels_like: current.apparent_temperature,
        code: current.weather_code,
        is_day: current.is_day != 0,
        humidity: current.relative_humidity_2m,
        wind: current.wind_speed_10m,
        high: forecast.daily.temperature_2m_max.first().copied().unwrap_or(current.temperature_2m),
        low: forecast.daily.temperature_2m_min.first().copied().unwrap_or(current.temperature_2m),
        hours,
    })
}

pub fn spawn(rt: &Handle, bus: EventBus) -> WeatherHandle {
    let (location_tx, mut location_rx) = watch::channel::<Option<Location>>(None);
    let (lookup_tx, mut lookup_rx) = mpsc::unbounded_channel::<String>();
    let lookup_bus = bus.clone();
    rt.spawn(async move {
        let client = client();
        while let Some(city) = lookup_rx.recv().await {
            let result = geocode(&client, &city).await;
            if let Err(err) = &result {
                log::warn!("weather: cannot find {city:?}: {err}");
            }
            lookup_bus.send(ServiceEvent::Weather(WeatherEvent::Located(result)));
        }
    });
    rt.spawn(async move {
        let client = client();
        loop {
            let location = location_rx.borrow_and_update().clone();
            let wait = match location {
                Some(location) => match fetch(&client, &location).await {
                    Ok(weather) => {
                        bus.send(ServiceEvent::Weather(WeatherEvent::Updated(weather)));
                        REFRESH
                    }
                    Err(err) => {
                        log::warn!("weather update failed: {err}");
                        bus.send(ServiceEvent::Weather(WeatherEvent::Failed(err)));
                        RETRY
                    }
                },
                None => Duration::MAX,
            };
            tokio::select! {
                _ = tokio::time::sleep(wait.min(Duration::from_secs(86_400))) => {}
                changed = location_rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    });
    WeatherHandle { location: location_tx, lookup: lookup_tx }
}

/// Short description of a WMO weather code.
pub fn describe(code: u8) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 | 63 => "Rain",
        65 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 | 77 => "Snow",
        80 | 81 => "Showers",
        82 => "Heavy showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm, hail",
        _ => "\u{2013}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hours() {
        assert_eq!(hour_of("2026-10-24T14:00"), Some(14));
        assert_eq!(hour_of("bad"), None);
        assert_eq!(describe(63), "Rain");
    }
}
