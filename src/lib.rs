use async_trait::async_trait;
use geo::{Centroid, MultiPoint, Point};
use protobuf::Message;
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use snafu::prelude::*;
use std::collections::HashMap;
use std::time::Duration;
use tokio::task::JoinSet;

mod protos {
    include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
}

const APPLE_LOC_API: &str = "https://gs-loc.apple.com/clls/wloc";

/// Apple rejects requests that carry more than a handful of BSSIDs, so large
/// batches are split into chunks of this size and sent concurrently.
const MAX_BSSID_PER_REQUEST: u8 = 10;

/// Apple encodes coordinates as fixed-point integers scaled by 1e8.
const COORD_SCALE: f64 = 1e-8;

/// Sentinel value Apple returns when it has no known location for a BSSID.
const INVALID_COORD: f64 = -180.0;

/// Every response body is prefixed with a 10-byte header before the protobuf payload.
const RESPONSE_HEADER_LEN: usize = 10;

/// Overall per-request timeout for the built-in client (so one hung request can't
/// stall the whole batch). Override it via [`Geolocation::from_client`].
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Connection-establishment timeout for the built-in client.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `num_wifi_results` value that makes Apple return only the requested BSSIDs.
const WIFI_RESULTS_REQUESTED_ONLY: i32 = 1;

/// `num_wifi_results` value that makes Apple also return neighbouring access points.
const WIFI_RESULTS_WITH_NEIGHBOURS: i32 = 0;

/// A geographic coordinate returned by the Apple location service.
///
/// Using named fields instead of a bare `(f64, f64)` makes it unambiguous which
/// value is which — `latitude` is north/south, `longitude` is east/west.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinate {
    pub latitude: f64,
    pub longitude: f64,
}

#[async_trait]
pub trait GeolocationApi {
    /// Resolves each BSSID to its coordinate. A present key with `None` means the
    /// server knows the BSSID but has no location for it.
    async fn get_coords(
        &self,
        bssids: Vec<String>,
    ) -> Result<HashMap<String, Option<Coordinate>>, Errors>;
    /// Like [`get_coords`](Self::get_coords), but also asks Apple for the
    /// neighbouring access points it knows about near the requested BSSIDs. The
    /// returned map therefore usually contains many more entries than were asked for.
    async fn get_coords_with_neighbours(
        &self,
        bssids: Vec<String>,
    ) -> Result<HashMap<String, Option<Coordinate>>, Errors>;
    /// Resolves the BSSIDs and returns the centroid of every known location.
    async fn get_centroid_coord(&self, bssids: Vec<String>) -> Result<Coordinate, Errors>;
}

pub struct Geolocation {
    client: reqwest::Client,
}

impl Default for Geolocation {
    /// Creates a default instance of `Geolocation`.
    ///
    /// This method initializes a `Geolocation` instance with a `reqwest` client that has gzip
    /// support enabled by default.
    ///
    /// # Notes
    ///
    /// Use this method when you do not need to configure specific options, such as a proxy.
    /// It provides a convenient way to create a `Geolocation` instance with sensible defaults.
    ///
    /// # Example
    ///
    /// ```
    /// use geolocation_apple::Geolocation;
    /// let geo = Geolocation::default();
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl Geolocation {
    /// Creates a new instance of `Geolocation`.
    ///
    /// This method initializes a `Geolocation` instance with a `reqwest` client that has gzip
    /// support enabled by default.
    ///
    /// # Notes
    ///
    /// Use this method when you do not need to configure specific options, such as a proxy.
    /// It provides a convenient way to create a `Geolocation` instance with sensible defaults.
    ///
    /// # Example
    ///
    /// ```
    /// use geolocation_apple::Geolocation;
    /// let geo = Geolocation::new();
    /// ```
    pub fn new() -> Self {
        let client = Client::builder()
            .gzip(true)
            .timeout(DEFAULT_TIMEOUT)
            .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
            .build()
            .expect("the default reqwest client should always build");
        Geolocation { client }
    }

    /// Creates a new instance of `Geolocation`.
    ///
    /// # Parameters
    ///
    /// * `client` - A `reqwest` client that will be used for making HTTP requests.
    ///
    /// # Notes
    ///
    /// This function requires that the client has gzip support. It is recommended to create
    /// an instance of `Geolocation` using this method only when you need to configure specific
    /// options, such as using a proxy. Otherwise, you can use the `::default`.
    ///
    /// # Example
    ///
    /// ```
    /// use geolocation_apple::Geolocation;
    /// use reqwest;
    /// let client = reqwest::Client::builder().gzip(true).build().unwrap();
    /// let geo = Geolocation::from_client(client);
    /// ```
    pub fn from_client(client: reqwest::Client) -> Self {
        Geolocation { client }
    }

    /// Sends the BSSID batch to Apple and merges every response into one map.
    ///
    /// `num_wifi_results` is forwarded to Apple: `1` returns only the requested
    /// BSSIDs, `0` also returns neighbouring access points. Bodies are built up
    /// front (so a serialization failure surfaces before any request is sent) and
    /// the chunks are then queried concurrently.
    async fn query(
        &self,
        bssids: Vec<String>,
        num_wifi_results: i32,
    ) -> Result<HashMap<String, Option<Coordinate>>, Errors> {
        let bodies: Vec<Vec<u8>> = bssids
            .chunks(MAX_BSSID_PER_REQUEST.into())
            .map(|chunk| {
                let mut apple_wloc = protos::AppleWLoc::AppleWLoc::new();
                for bssid in chunk {
                    let mut wifi_device = protos::AppleWLoc::WifiDevice::new();
                    wifi_device.set_bssid(bssid.to_string());
                    apple_wloc.wifi_devices.push(wifi_device);
                }
                apple_wloc.set_num_cell_results(0);
                apple_wloc.set_num_wifi_results(num_wifi_results);

                let payload = apple_wloc
                    .write_to_bytes()
                    .expect("AppleWLoc with a required bssid always serializes");
                build_request_body(&payload)
            })
            .collect();

        let requests: JoinSet<_> = bodies
            .into_iter()
            .map(|body| {
                self.client
                    .post(APPLE_LOC_API)
                    .headers(create_headers())
                    .body(body)
                    .send()
            })
            .collect();

        let mut result = HashMap::new();
        for response in requests.join_all().await {
            let response = response.map_err(|_| Errors::ConnectionError)?;
            if !response.status().is_success() {
                return Err(Errors::ResponseError {
                    code: response.status().as_u16(),
                });
            }

            let body = response
                .bytes()
                .await
                .map_err(|_| Errors::BadResponseError)?;
            if body.len() < RESPONSE_HEADER_LEN {
                return Err(Errors::BadResponseError);
            }

            let mut apple_wloc = protos::AppleWLoc::AppleWLoc::new();
            apple_wloc
                .merge_from_bytes(&body[RESPONSE_HEADER_LEN..])
                .map_err(|_| Errors::BadResponseError)?;
            result.extend(process_result(&apple_wloc));
        }
        Ok(result)
    }
}

/// Builds the fixed set of headers Apple's `locationd` sends with every request.
fn create_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Content-Type",
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert("Accept", HeaderValue::from_static("*/*"));
    headers.insert("Accept-Language", HeaderValue::from_static("en-us"));
    headers.insert("Accept-Charset", HeaderValue::from_static("utf-8"));
    headers.insert("Accept-Encoding", HeaderValue::from_static("gzip, deflate"));
    headers.insert(
        "User-Agent",
        HeaderValue::from_static("locationd/1753.17 CFNetwork/711.1.12 Darwin/14.0.0"),
    );
    headers
}

/// Normalizes a BSSID to a canonical form: lowercase, with every colon-separated
/// octet padded to two hex digits (Apple returns single-digit octets without a
/// leading zero). Lowercasing keeps map keys consistent regardless of input case.
fn format_bssid(bssid: &str) -> String {
    bssid
        .split(':')
        .map(|octet| {
            if octet.len() == 1 {
                format!("0{}", octet.to_lowercase())
            } else {
                octet.to_lowercase()
            }
        })
        .collect::<Vec<String>>()
        .join(":")
}

/// Wraps the serialized `AppleWLoc` protobuf `payload` in the binary envelope
/// (locale, bundle id and version) that the `gs-loc.apple.com` endpoint expects.
fn build_request_body(payload: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(50 + payload.len());
    data.extend_from_slice(&[0x00, 0x01, 0x00, 0x05]);
    data.extend_from_slice(b"en_US");
    data.extend_from_slice(&[0x00, 0x13]);
    data.extend_from_slice(b"com.apple.locationd");
    data.extend_from_slice(&[0x00, 0x0a]);
    data.extend_from_slice(b"8.1.12B411");
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]);
    data.push(payload.len() as u8);
    data.extend_from_slice(payload);
    data
}

/// Turns a decoded `AppleWLoc` response into a `bssid -> coordinate` map. Devices
/// without a location are skipped; the `-180.0` sentinel is mapped to `None`.
fn process_result(
    apple_wloc: &protos::AppleWLoc::AppleWLoc,
) -> HashMap<String, Option<Coordinate>> {
    let mut device_locations = HashMap::new();
    for wifi_device in &apple_wloc.wifi_devices {
        let (Some(longitude), Some(latitude)) = (
            wifi_device.location.longitude,
            wifi_device.location.latitude,
        ) else {
            continue;
        };
        let latitude = latitude as f64 * COORD_SCALE;
        let longitude = longitude as f64 * COORD_SCALE;
        let coord =
            (latitude != INVALID_COORD && longitude != INVALID_COORD).then_some(Coordinate {
                latitude,
                longitude,
            });
        device_locations.insert(format_bssid(wifi_device.bssid()), coord);
    }
    device_locations
}

#[derive(Debug, Snafu)]
pub enum Errors {
    #[snafu(display("Can't connect to the BSSID location server"))]
    ConnectionError,

    #[snafu(display("The BSSID location server responded with an error (code: {code})"))]
    ResponseError { code: u16 },

    #[snafu(display("Can't calculate a centroid for these BSSIDs"))]
    CalculationError,

    #[snafu(display("The BSSID location server returned a malformed response"))]
    BadResponseError,
}

#[async_trait]
impl GeolocationApi for Geolocation {
    async fn get_coords(
        &self,
        bssids: Vec<String>,
    ) -> Result<HashMap<String, Option<Coordinate>>, Errors> {
        self.query(bssids, WIFI_RESULTS_REQUESTED_ONLY).await
    }

    async fn get_coords_with_neighbours(
        &self,
        bssids: Vec<String>,
    ) -> Result<HashMap<String, Option<Coordinate>>, Errors> {
        self.query(bssids, WIFI_RESULTS_WITH_NEIGHBOURS).await
    }

    async fn get_centroid_coord(&self, bssids: Vec<String>) -> Result<Coordinate, Errors> {
        let coords = self.get_coords(bssids).await?;
        let points: MultiPoint<_> = coords
            .values()
            .copied()
            .flatten()
            .map(|coord| Point::new(coord.latitude, coord.longitude))
            .collect::<Vec<Point>>()
            .into();

        points
            .centroid()
            .map(|centroid| Coordinate {
                latitude: centroid.x(),
                longitude: centroid.y(),
            })
            .ok_or(Errors::CalculationError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::MessageField;

    #[test]
    fn format_bssid_pads_single_digit_octets() {
        assert_eq!(format_bssid("0:1:2:3:4:5"), "00:01:02:03:04:05");
        assert_eq!(format_bssid("a:bb:c:dd:e:ff"), "0a:bb:0c:dd:0e:ff");
    }

    #[test]
    fn format_bssid_leaves_full_octets_untouched() {
        assert_eq!(format_bssid("00:11:22:33:44:55"), "00:11:22:33:44:55");
    }

    #[test]
    fn format_bssid_lowercases() {
        assert_eq!(format_bssid("AA:BB:CC:DD:EE:FF"), "aa:bb:cc:dd:ee:ff");
        assert_eq!(format_bssid("A:B:C:D:E:F"), "0a:0b:0c:0d:0e:0f");
    }

    #[test]
    fn build_request_body_frames_payload() {
        let payload = b"hello";
        let body = build_request_body(payload);

        // Fixed 50-byte envelope followed by the payload.
        assert_eq!(body.len(), 50 + payload.len());
        assert_eq!(&body[..4], &[0x00, 0x01, 0x00, 0x05]);
        assert_eq!(&body[4..9], b"en_US");
        // The last envelope byte carries the payload length.
        assert_eq!(body[49], payload.len() as u8);
        assert_eq!(&body[50..], payload);
    }

    fn wifi_device(bssid: &str, coord: Option<(i64, i64)>) -> protos::AppleWLoc::WifiDevice {
        let mut device = protos::AppleWLoc::WifiDevice::new();
        device.set_bssid(bssid.to_string());
        if let Some((lat, lon)) = coord {
            let mut location = protos::AppleWLoc::wifi_device::Location::new();
            location.set_latitude(lat);
            location.set_longitude(lon);
            device.location = MessageField::some(location);
        }
        device
    }

    #[test]
    fn process_result_scales_formats_and_filters() {
        let mut apple_wloc = protos::AppleWLoc::AppleWLoc::new();
        // Valid coordinate: 50.0 / 37.0 after scaling, BSSID gets padded.
        apple_wloc.wifi_devices.push(wifi_device(
            "1:2:3:4:5:6",
            Some((5_000_000_000, 3_700_000_000)),
        ));
        // Sentinel -180.0 -> mapped to None.
        apple_wloc.wifi_devices.push(wifi_device(
            "aa:bb:cc:dd:ee:ff",
            Some((-18_000_000_000, -18_000_000_000)),
        ));
        // No location -> skipped entirely.
        apple_wloc
            .wifi_devices
            .push(wifi_device("11:22:33:44:55:66", None));

        let result = process_result(&apple_wloc);

        assert_eq!(result.len(), 2);
        assert_eq!(
            result.get("01:02:03:04:05:06"),
            Some(&Some(Coordinate {
                latitude: 50.0,
                longitude: 37.0,
            }))
        );
        assert_eq!(result.get("aa:bb:cc:dd:ee:ff"), Some(&None));
        assert!(!result.contains_key("11:22:33:44:55:66"));
    }
}
