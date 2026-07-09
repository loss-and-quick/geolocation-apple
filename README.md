# geolocation-apple

Resolve geographic coordinates for Wi-Fi access points (BSSIDs) using Apple's
Wi-Fi Positioning Service — the same undocumented `gs-loc.apple.com/clls/wloc`
endpoint that `locationd` on iOS/macOS talks to. Ships as both a **library** and
a small **CLI**.

> [!WARNING]
> This uses a private, undocumented Apple endpoint and is meant for research and
> educational use. Query only access points you are authorized to look up, and
> respect the `_nomap` SSID opt-out. Coordinates come from Apple's crowd-sourced
> database and can be stale or imprecise.

## How it works

For each BSSID the client sends a protobuf request (wrapped in Apple's binary
envelope) and parses the response:

- Coordinates are fixed-point integers **scaled by `1e8`** (e.g. `5986726760` →
  `59.86726760`).
- An unknown BSSID comes back with the sentinel `latitude/longitude = -180` and
  `horizontal_accuracy = -1`; the library maps that to `None`.
- With `num_wifi_results = 1` Apple returns only the requested BSSIDs. Set it to
  `0` and Apple also returns up to ~100 *neighbouring* access points.

The response field layout (`horizontal_accuracy`, `altitude`, `vertical_accuracy`,
…) is documented inline in [`protos/AppleWLoc.proto`](protos/AppleWLoc.proto),
based on the reverse engineering in
[acheong08/apple-corelocation-experiments](https://github.com/acheong08/apple-corelocation-experiments).

## Requirements

- Rust **1.96.1** (pinned in [`rust-toolchain.toml`](rust-toolchain.toml)).
- A Nix dev shell is provided; `nix develop` gives you the exact toolchain plus
  `rustfmt`/`clippy`. Plain `cargo` works too if you have the toolchain.

```sh
nix develop            # drop into the pinned toolchain
cargo build --release
```

## CLI usage

```sh
# One or more BSSIDs (accepts ':' or '-' separators, any case)
geolocation-apple a4:2b:8c:11:22:33 a4:2b:8c:11:22:34

# Approximate a single position as the centroid of all known APs
geolocation-apple -c a4:2b:8c:11:22:33 a4:2b:8c:11:22:34
```

Example output (coordinates illustrative):

```text
a4:2b:8c:11:22:33: 59.86726760 30.30793952
a4:2b:8c:11:22:34: not found
```

```text
Centroid: 59.86728858 30.30803966
```

`not found` means Apple has no location for that BSSID (or it isn't in the
database yet).

### Getting real BSSIDs to test with

On Linux, `nmcli dev wifi list` may **mask** the real BSSIDs (you'll see fake
sequential MACs like `00:01:02:00:00:0X`). Read them straight from the driver
instead:

```sh
iw dev wlan0 scan | grep -E '^BSS'
```

## Library usage

```toml
[dependencies]
geolocation-apple = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

```rust
use geolocation_apple::{Geolocation, GeolocationApi};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let geo = Geolocation::default();

    // Per-BSSID lookup. `Some(None)` = known BSSID with no location.
    let coords = geo.get_coords(vec!["a4:2b:8c:11:22:33".to_string()]).await?;
    for (bssid, location) in &coords {
        match location {
            Some(c) => println!("{bssid}: {:.6}, {:.6}", c.latitude, c.longitude),
            None => println!("{bssid}: known, no location"),
        }
    }

    // Centroid of every known access point in the batch.
    let centre = geo
        .get_centroid_coord(vec![
            "a4:2b:8c:11:22:33".to_string(),
            "a4:2b:8c:11:22:34".to_string(),
        ])
        .await?;
    println!("centroid: {:.6}, {:.6}", centre.latitude, centre.longitude);

    Ok(())
}
```

### Custom client (proxies, timeouts)

`Geolocation::default()` builds a gzip-enabled client. To route through a proxy
or tweak timeouts, pass your own client — it **must** have gzip enabled:

```rust
use geolocation_apple::Geolocation;

let client = reqwest::Client::builder()
    .gzip(true)
    .proxy(reqwest::Proxy::all("http://127.0.0.1:10808")?)
    .build()?;
let geo = Geolocation::from_client(client);
```

### API surface

- `Geolocation::new()` / `::default()` — client with gzip enabled.
- `Geolocation::from_client(reqwest::Client)` — bring your own client.
- `trait GeolocationApi`:
  - `get_coords(Vec<String>) -> Result<HashMap<String, Option<Coordinate>>, Errors>`
  - `get_centroid_coord(Vec<String>) -> Result<Coordinate, Errors>`
- `struct Coordinate { latitude: f64, longitude: f64 }` — named fields so it's
  never ambiguous which value is which.
- `enum Errors` — `ConnectionError`, `ResponseError { code }`,
  `CalculationError`, `BadResponseError`.

## Development

```sh
cargo test          # unit + doc tests
cargo fmt
cargo clippy --all-targets -- -D warnings
```

Layout:

```
build.rs                 # compiles the .proto at build time (protobuf-codegen)
protos/AppleWLoc.proto   # request/response schema with field semantics
src/lib.rs               # library: request framing, response parsing, API
src/main.rs              # CLI
flake.nix                # pinned Rust dev shell (rust-overlay)
```

## Credits

Protobuf field semantics from
[acheong08/apple-corelocation-experiments](https://github.com/acheong08/apple-corelocation-experiments).
