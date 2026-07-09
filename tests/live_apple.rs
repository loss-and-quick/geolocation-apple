//! Integration tests that hit the real Apple endpoint. Ignored by default because
//! they require network access and a BSSID that actually exists in Apple's
//! database. Run them explicitly, e.g.:
//!
//! ```sh
//! APPLE_TEST_BSSID=de:53:2a:72:13:a8 cargo test --test live_apple -- --ignored --nocapture
//! ```

use geolocation_apple::{Geolocation, GeolocationApi};

fn seed_bssid() -> String {
    std::env::var("APPLE_TEST_BSSID")
        .expect("set APPLE_TEST_BSSID to a real, in-database BSSID to run the live tests")
}

#[tokio::test]
#[ignore = "hits the live Apple endpoint; set APPLE_TEST_BSSID"]
async fn live_lookup_returns_a_coordinate() {
    let bssid = seed_bssid();
    let geo = Geolocation::default();

    let coords = geo
        .get_coords(vec![bssid.clone()])
        .await
        .expect("live request to Apple failed");

    // The requested BSSID must be present (keys are normalised to lowercase).
    let location = coords
        .get(&bssid.to_lowercase())
        .expect("requested BSSID missing from the response");
    let coord = location.expect("Apple has no location for the seed BSSID");

    assert!(
        (-90.0..=90.0).contains(&coord.latitude),
        "latitude out of range: {}",
        coord.latitude
    );
    assert!(
        (-180.0..=180.0).contains(&coord.longitude),
        "longitude out of range: {}",
        coord.longitude
    );
}

#[tokio::test]
#[ignore = "hits the live Apple endpoint; set APPLE_TEST_BSSID"]
async fn live_neighbours_returns_more_than_requested() {
    let bssid = seed_bssid();
    let geo = Geolocation::default();

    let neighbours = geo
        .get_coords_with_neighbours(vec![bssid])
        .await
        .expect("live neighbour request to Apple failed");

    // A seed that is in the database should surface its neighbouring APs too.
    assert!(
        neighbours.len() > 1,
        "expected neighbouring APs, got {} entries",
        neighbours.len()
    );
}
