use crate::tool_bridge::{StreamAuth, ToolBridge};
use agentic_protocol::ChatStreamEvent;
use agentic_tools::{
    DELETE_DIRECTORY, DELETE_FILE, EDIT_FILE, LIST_FILES, READ_FILE, SEARCH_FILES, ToolExecution,
    WRITE_FILE,
};
use rig::{completion::ToolDefinition, tool::Tool};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};
use tokio::sync::mpsc;

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
    const NAME: &'static str = agentic_tools::GET_CURRENT_WEATHER;
    type Error = WeatherToolError;
    type Args = WeatherArgs;
    type Output = WeatherReport;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let spec = agentic_tools::spec(Self::NAME).expect("weather tool spec exists");
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: spec.description.to_owned(),
            parameters: (spec.parameters)(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        current_weather(args).await
    }
}

#[derive(Debug, Clone)]
pub struct LocalToolContext {
    bridge: ToolBridge,
    auth: StreamAuth,
    events: mpsc::UnboundedSender<ChatStreamEvent>,
}

impl LocalToolContext {
    pub fn new(
        bridge: ToolBridge,
        auth: StreamAuth,
        events: mpsc::UnboundedSender<ChatStreamEvent>,
    ) -> Self {
        Self {
            bridge,
            auth,
            events,
        }
    }

    async fn call_tool(
        &self,
        name: &'static str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ProxyToolError> {
        let spec = agentic_tools::spec(name).ok_or_else(|| ProxyToolError {
            message: format!("unknown local tool '{name}'"),
        })?;
        let ToolExecution::LocalProxy { approval_required } = spec.execution else {
            return Err(ProxyToolError {
                message: format!("'{name}' is not a local proxy tool"),
            });
        };
        let summary = local_tool_summary(name, &input);
        self.bridge
            .request_local_tool(
                &self.auth,
                &self.events,
                name.to_owned(),
                input,
                approval_required,
                summary,
            )
            .await
            .map_err(|error| ProxyToolError {
                message: error.to_string(),
            })
    }
}

#[derive(Debug)]
pub struct ProxyToolError {
    message: String,
}

impl fmt::Display for ProxyToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ProxyToolError {}

macro_rules! proxy_tool {
    ($type_name:ident, $tool_name:expr) => {
        #[derive(Debug, Clone)]
        pub struct $type_name {
            context: LocalToolContext,
        }

        impl $type_name {
            pub fn new(context: LocalToolContext) -> Self {
                Self { context }
            }
        }

        impl Tool for $type_name {
            const NAME: &'static str = $tool_name;
            type Error = ProxyToolError;
            type Args = serde_json::Value;
            type Output = serde_json::Value;

            async fn definition(&self, _prompt: String) -> ToolDefinition {
                let spec = agentic_tools::spec(Self::NAME).expect("proxy tool spec exists");
                ToolDefinition {
                    name: Self::NAME.to_owned(),
                    description: spec.description.to_owned(),
                    parameters: (spec.parameters)(),
                }
            }

            async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
                self.context.call_tool(Self::NAME, args).await
            }
        }
    };
}

proxy_tool!(ListFilesProxyTool, LIST_FILES);
proxy_tool!(ReadFileProxyTool, READ_FILE);
proxy_tool!(SearchFilesProxyTool, SEARCH_FILES);
proxy_tool!(WriteFileProxyTool, WRITE_FILE);
proxy_tool!(EditFileProxyTool, EDIT_FILE);
proxy_tool!(DeleteFileProxyTool, DELETE_FILE);
proxy_tool!(DeleteDirectoryProxyTool, DELETE_DIRECTORY);

fn local_tool_summary(name: &str, input: &serde_json::Value) -> String {
    let path = input
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(".");
    match name {
        LIST_FILES => format!("List files under {path}"),
        READ_FILE => format!("Read {path}"),
        SEARCH_FILES => {
            let query = input
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            format!("Search {path} for '{query}'")
        }
        WRITE_FILE => format!("Write {path}"),
        EDIT_FILE => format!("Edit {path}"),
        DELETE_FILE => format!("Delete {path}"),
        DELETE_DIRECTORY => format!("Delete directory {path}"),
        _ => format!("Run {name}"),
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
        Some(1..=3) => "Partly cloudy",
        Some(45 | 48) => "Foggy",
        Some(51 | 53 | 55 | 56 | 57) => "Drizzle",
        Some(61 | 63 | 65 | 66 | 67) => "Rain",
        Some(71 | 73 | 75 | 77) => "Snow",
        Some(80..=82) => "Rain showers",
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
