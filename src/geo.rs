//! Distance between HQ and mesh nodes.
//!
//! Node positions are self-reported (advert GPS) and HQ's anchor is either the
//! operator's site pin or the HQ companion's own advert position. Both are
//! ignored when they are missing, out of range, or the 0,0 "no fix" placeholder.

use crate::db::Db;

const EARTH_RADIUS_M: f64 = 6_371_000.0;
const METERS_PER_MILE: f64 = 1_609.344;
const FEET_PER_METER: f64 = 3.280_84;

/// True for a real fix: finite, in range, and not the 0,0 placeholder.
pub fn valid_fix(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
        && (lat.abs() > 0.0001 || lon.abs() > 0.0001)
}

/// Great-circle distance in meters (haversine).
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let d_phi = (lat2 - lat1).to_radians();
    let d_lambda = (lon2 - lon1).to_radians();
    let a = (d_phi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (d_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().min(1.0).asin()
}

/// HQ's reference position: the operator's site pin first (explicit), then the
/// HQ companion's advert position.
pub fn hq_anchor(db: &Db) -> Option<(f64, f64)> {
    let site_lat = crate::services::setting_f64(db, "hq_site_lat", f64::NAN);
    let site_lon = crate::services::setting_f64(db, "hq_site_lon", f64::NAN);
    if valid_fix(site_lat, site_lon) {
        return Some((site_lat, site_lon));
    }
    db.hq_position()
        .ok()
        .flatten()
        .filter(|(_, lat, lon)| valid_fix(*lat, *lon))
        .map(|(_, lat, lon)| (lat, lon))
}

/// Human distance: feet/miles when `imperial`, otherwise meters/kilometers.
pub fn format_distance_m(meters: f64, imperial: bool) -> String {
    if imperial {
        let miles = meters / METERS_PER_MILE;
        if miles < 0.1 {
            format!("{} ft", (meters * FEET_PER_METER).round() as i64)
        } else if miles < 10.0 {
            format!("{miles:.1} mi")
        } else {
            format!("{:.0} mi", miles)
        }
    } else if meters < 1_000.0 {
        format!("{} m", meters.round() as i64)
    } else {
        let km = meters / 1_000.0;
        if km < 10.0 {
            format!("{km:.1} km")
        } else {
            format!("{km:.0} km")
        }
    }
}

/// Distance label for a node position, or `None` when either side lacks a fix.
pub fn distance_label(
    anchor: Option<(f64, f64)>,
    lat: Option<f64>,
    lon: Option<f64>,
    imperial: bool,
) -> Option<String> {
    let (alat, alon) = anchor?;
    let (lat, lon) = (lat?, lon?);
    if !valid_fix(lat, lon) {
        return None;
    }
    Some(format_distance_m(haversine_m(alat, alon, lat, lon), imperial))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_known_distance() {
        // Los Angeles City Hall to San Diego downtown ≈ 179 km.
        let d = haversine_m(34.0537, -118.2428, 32.7157, -117.1611);
        assert!((d / 1000.0 - 179.0).abs() < 3.0, "{d}");
        assert!(haversine_m(10.0, 10.0, 10.0, 10.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_zero_and_out_of_range_fixes() {
        assert!(!valid_fix(0.0, 0.0));
        assert!(!valid_fix(91.0, 0.0));
        assert!(!valid_fix(0.0, 181.0));
        assert!(!valid_fix(f64::NAN, 1.0));
        assert!(valid_fix(34.05, -118.24));
    }

    #[test]
    fn formats_metric_and_imperial() {
        assert_eq!(format_distance_m(420.0, false), "420 m");
        assert_eq!(format_distance_m(4_200.0, false), "4.2 km");
        assert_eq!(format_distance_m(42_000.0, false), "42 km");
        assert_eq!(format_distance_m(4_800.0, true), "3.0 mi");
        assert_eq!(format_distance_m(17_000.0, true), "11 mi");
        assert_eq!(format_distance_m(30.0, true), "98 ft");
    }

    #[test]
    fn label_needs_both_fixes() {
        let anchor = Some((34.0, -118.0));
        assert!(distance_label(anchor, Some(0.0), Some(0.0), false).is_none());
        assert!(distance_label(None, Some(34.1), Some(-118.0), false).is_none());
        assert!(distance_label(anchor, None, None, false).is_none());
        let label = distance_label(anchor, Some(34.1), Some(-118.0), false).unwrap();
        assert!(label.ends_with(" km") || label.ends_with(" m"), "{label}");
    }
}
