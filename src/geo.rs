//! Place search via OpenStreetMap Nominatim.
//!
//! Usage policy (https://operations.osmfoundation.org/policies/nominatim/): max. 1 request per
//! second, an own User-Agent, OSM attribution, no search-as-you-type. The app only searches on
//! Enter/button, throttles requests and caches results.

use crate::model::{tr, url_encode};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct GeoHit {
    pub lat: f64,
    pub lon: f64,
    pub city: String,
    pub country: String,
    pub display: String,
}

fn user_agent() -> String {
    let repo = env!("CARGO_PKG_REPOSITORY");
    if repo.is_empty() {
        format!("Rollstamp/{}", env!("CARGO_PKG_VERSION"))
    } else {
        format!("Rollstamp/{} (+{repo})", env!("CARGO_PKG_VERSION"))
    }
}

fn get_json(url: &str) -> Result<Value, String> {
    let ua = user_agent();
    let mut resp = ureq::get(url).header("User-Agent", ua.as_str()).call().map_err(|e| {
        format!(
            "{}: {e}",
            tr("Ortssuche nicht erreichbar", "Place search not reachable")
        )
    })?;
    let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

fn hit_from(v: &Value) -> Option<GeoHit> {
    let coord = |k: &str| v.get(k).and_then(|x| x.as_str()).and_then(|s| s.parse::<f64>().ok());
    let lat = coord("lat")?;
    let lon = coord("lon")?;
    let addr = v.get("address");
    let a = |k: &str| {
        addr.and_then(|a| a.get(k))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string()
    };
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let city = [
        "city",
        "town",
        "village",
        "municipality",
        "island",
        "hamlet",
        "suburb",
        "county",
    ]
    .iter()
    .map(|k| a(k))
    .find(|s| !s.is_empty())
    .unwrap_or(name);
    Some(GeoHit {
        lat,
        lon,
        city,
        country: a("country"),
        display: v.get("display_name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
    })
}

/// Search by name, e.g. "Borkum". `lang`: "de" or "en" (language of the results).
pub fn search(query: &str, lang: &str) -> Result<Vec<GeoHit>, String> {
    let url = format!(
        "https://nominatim.openstreetmap.org/search?format=jsonv2&addressdetails=1&limit=8&accept-language={lang}&q={}",
        url_encode(query.trim())
    );
    let v = get_json(&url)?;
    let arr = v.as_array().ok_or_else(|| {
        tr(
            "Unerwartete Antwort der Ortssuche.",
            "Unexpected answer from place search.",
        )
        .to_string()
    })?;
    Ok(arr.iter().filter_map(hit_from).collect())
}

/// Place name for coordinates.
pub fn reverse(lat: f64, lon: f64, lang: &str) -> Result<GeoHit, String> {
    let url = format!(
        "https://nominatim.openstreetmap.org/reverse?format=jsonv2&addressdetails=1&zoom=14&accept-language={lang}&lat={lat:.6}&lon={lon:.6}"
    );
    let v = get_json(&url)?;
    if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
        return Err(e.to_string());
    }
    hit_from(&v).ok_or_else(|| tr("Kein Ort gefunden.", "No place found.").to_string())
}

/// Link to check a position on openstreetmap.org.
pub fn map_url(lat: f64, lon: f64) -> String {
    format!("https://www.openstreetmap.org/?mlat={lat:.6}&mlon={lon:.6}#map=15/{lat:.6}/{lon:.6}")
}
