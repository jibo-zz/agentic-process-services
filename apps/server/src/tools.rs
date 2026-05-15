use rig::{completion::ToolDefinition, tool::Tool};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize)]
pub struct CurrentWeatherTool;

#[derive(Debug, Deserialize)]
pub struct WeatherArgs {
    /// City name to look up, for example Berlin or San Francisco.
    city: String,
    /// Optional country name or country code to disambiguate the city.
    country: Option<String>,
    /// Optional temperature unit. Defaults to celsius. Use fahrenheit only when explicitly requested.
    unit: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WeatherReport {
    location: String,
    temperature: f64,
    unit: String,
    apparent_temperature: Option<f64>,
    relative_humidity: Option<f64>,
    wind_speed: Option<f64>,
    weather_code: Option<i64>,
    summary: String,
}

#[derive(Debug)]
pub struct WeatherToolError(String);

impl WeatherToolError {
    fn city_not_found(city: &str, country: Option<&str>) -> Self {
        let message = match country {
            Some(country) => format!(
                "Could not find a city named '{city}' in '{country}'. Please check the spelling."
            ),
            None => format!("Could not find a city named '{city}'. Please check the spelling."),
        };
        Self(message)
    }

    fn lookup_failed(city: &str) -> Self {
        Self(format!(
            "Weather lookup failed for '{city}'. Please try again later."
        ))
    }
}

impl fmt::Display for WeatherToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for WeatherToolError {}

impl Tool for CurrentWeatherTool {
    const NAME: &'static str = "get_current_weather";
    type Error = WeatherToolError;
    type Args = WeatherArgs;
    type Output = WeatherReport;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description:
                "Get the current weather for a city. Use this for current weather questions."
                    .to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string",
                        "description": "City name, for example Berlin or San Francisco."
                    },
                    "country": {
                        "type": "string",
                        "description": "Optional country name or country code for disambiguation."
                    },
                    "unit": {
                        "type": "string",
                        "description": "Temperature unit. Defaults to celsius. Use fahrenheit only when explicitly requested.",
                        "enum": ["celsius", "fahrenheit"]
                    }
                },
                "required": ["city"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        current_weather(args).await
    }
}

async fn current_weather(args: WeatherArgs) -> Result<WeatherReport, WeatherToolError> {
    let unit = TemperatureUnit::from_arg(args.unit.as_deref());
    let client = reqwest::Client::new();
    let place = geocode(&client, &args.city, args.country.as_deref()).await?;
    let weather = fetch_weather(&client, &args.city, &place, unit).await?;
    Ok(weather)
}

async fn geocode(
    client: &reqwest::Client,
    city: &str,
    country: Option<&str>,
) -> Result<GeocodeResult, WeatherToolError> {
    let response = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[
            ("name", city),
            ("count", "5"),
            ("language", "en"),
            ("format", "json"),
        ])
        .send()
        .await
        .map_err(|_| WeatherToolError::lookup_failed(city))?;

    if !response.status().is_success() {
        return Err(WeatherToolError::lookup_failed(city));
    }

    let body: GeocodeResponse = response
        .json()
        .await
        .map_err(|_| WeatherToolError::lookup_failed(city))?;
    let Some(results) = body.results else {
        return Err(WeatherToolError::city_not_found(city, country));
    };

    results
        .iter()
        .find(|result| country_matches(result, country))
        .or_else(|| results.first())
        .cloned()
        .ok_or_else(|| WeatherToolError::city_not_found(city, country))
}

async fn fetch_weather(
    client: &reqwest::Client,
    city: &str,
    place: &GeocodeResult,
    unit: TemperatureUnit,
) -> Result<WeatherReport, WeatherToolError> {
    let latitude = place.latitude.to_string();
    let longitude = place.longitude.to_string();
    let response = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", latitude.as_str()),
            ("longitude", longitude.as_str()),
            (
                "current",
                "temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code",
            ),
            ("temperature_unit", unit.as_query_value()),
        ])
        .send()
        .await
        .map_err(|_| WeatherToolError::lookup_failed(city))?;

    if !response.status().is_success() {
        return Err(WeatherToolError::lookup_failed(city));
    }

    let body: WeatherResponse = response
        .json()
        .await
        .map_err(|_| WeatherToolError::lookup_failed(city))?;
    let current = body
        .current
        .ok_or_else(|| WeatherToolError::lookup_failed(city))?;
    let temperature = current
        .temperature_2m
        .ok_or_else(|| WeatherToolError::lookup_failed(city))?;
    let weather_code = current.weather_code;
    let summary = weather_summary(weather_code);

    Ok(WeatherReport {
        location: place.display_name(),
        temperature,
        unit: unit.label().to_owned(),
        apparent_temperature: current.apparent_temperature,
        relative_humidity: current.relative_humidity_2m,
        wind_speed: current.wind_speed_10m,
        weather_code,
        summary,
    })
}

fn country_matches(result: &GeocodeResult, country: Option<&str>) -> bool {
    let Some(country) = country else {
        return true;
    };
    result
        .country
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case(country))
        || result
            .country_code
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(country))
}

fn weather_summary(code: Option<i64>) -> String {
    match code {
        Some(0) => "Clear sky",
        Some(1 | 2 | 3) => "Partly cloudy",
        Some(45 | 48) => "Foggy",
        Some(51 | 53 | 55 | 56 | 57) => "Drizzle",
        Some(61 | 63 | 65 | 66 | 67) => "Rain",
        Some(71 | 73 | 75 | 77) => "Snow",
        Some(80 | 81 | 82) => "Rain showers",
        Some(85 | 86) => "Snow showers",
        Some(95 | 96 | 99) => "Thunderstorm",
        Some(_) => "Weather conditions unavailable",
        None => "Weather conditions unavailable",
    }
    .to_owned()
}

#[derive(Clone, Copy)]
enum TemperatureUnit {
    Celsius,
    Fahrenheit,
}

impl TemperatureUnit {
    fn from_arg(unit: Option<&str>) -> Self {
        match unit.map(str::to_ascii_lowercase).as_deref() {
            Some("fahrenheit" | "f" | "imperial") => Self::Fahrenheit,
            _ => Self::Celsius,
        }
    }

    fn as_query_value(self) -> &'static str {
        match self {
            Self::Celsius => "celsius",
            Self::Fahrenheit => "fahrenheit",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Celsius => "celsius",
            Self::Fahrenheit => "fahrenheit",
        }
    }
}

#[derive(Debug, Deserialize)]
struct GeocodeResponse {
    results: Option<Vec<GeocodeResult>>,
}

#[derive(Debug, Clone, Deserialize)]
struct GeocodeResult {
    name: String,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
    country_code: Option<String>,
    admin1: Option<String>,
}

impl GeocodeResult {
    fn display_name(&self) -> String {
        [
            Some(self.name.as_str()),
            self.admin1.as_deref(),
            self.country.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ")
    }
}

#[derive(Debug, Deserialize)]
struct WeatherResponse {
    current: Option<CurrentWeather>,
}

#[derive(Debug, Deserialize)]
struct CurrentWeather {
    temperature_2m: Option<f64>,
    apparent_temperature: Option<f64>,
    relative_humidity_2m: Option<f64>,
    wind_speed_10m: Option<f64>,
    weather_code: Option<i64>,
}
