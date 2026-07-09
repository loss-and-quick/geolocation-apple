use clap::{arg, command};
use geolocation_apple::{Geolocation, GeolocationApi};
use regex::Regex;
use std::process::ExitCode;
use std::sync::LazyLock;

/// Compiled once and reused for every value clap validates.
static BSSID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9A-Fa-f]{2}[:-]){5}([0-9A-Fa-f]{2})$").unwrap());

fn bssid_parser(bssid: &str) -> Result<String, String> {
    if BSSID_RE.is_match(bssid) {
        Ok(bssid.to_string())
    } else {
        Err(format!("'{bssid}' is NOT a valid BSSID."))
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = command!()
        .author("minicx, minicx@disroot.org")
        .about("Retrieve geographical coordinates for specified BSSID values.")
        .long_about("This program retrieves the geographical coordinates for a list of BSSID values (MAC addresses). You can specify between 1 and 24 BSSIDs. The output format is: 'text(BSSID for etc): latitude longitude'.")
        .arg(
            arg!(<BSSIDS> "A list of BSSID values (MAC addresses) to retrieve their geographical coordinates. You can specify between 1 and 24 BSSIDs.")
                .num_args(1..)
                .value_delimiter(' ')
                .value_parser(bssid_parser)
                .required(true)
                .action(clap::ArgAction::Set),
        )
        .arg(
            arg!(-c --centroid "Find approximate location using centroid algorithm").action(clap::ArgAction::SetTrue)
        )
        .arg(
            arg!(-n --neighbours "Also list neighbouring access points Apple knows about nearby").action(clap::ArgAction::SetTrue)
        )
        .get_matches();

    let bssids: Vec<String> = args
        .get_many::<String>("BSSIDS")
        .unwrap()
        .map(|s| s.to_owned())
        .collect();
    let geolocation = Geolocation::default();

    if args.get_flag("centroid") {
        let coord = geolocation.get_centroid_coord(bssids).await?;
        println!("Centroid: {:.8} {:.8}", coord.latitude, coord.longitude);
    } else if args.get_flag("neighbours") {
        // Print everything Apple returned, including the neighbouring APs that
        // weren't asked for, sorted for stable output.
        let coords = geolocation.get_coords_with_neighbours(bssids).await?;
        let mut entries: Vec<_> = coords.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (bssid, location) in entries {
            match location {
                Some(coord) => println!("{bssid}: {:.8} {:.8}", coord.latitude, coord.longitude),
                None => println!("{bssid}: not found"),
            }
        }
    } else {
        let coords = geolocation.get_coords(bssids.clone()).await?;
        for bssid in &bssids {
            match coords.get(&bssid.to_lowercase()) {
                Some(Some(coord)) => {
                    println!("{bssid}: {:.8} {:.8}", coord.latitude, coord.longitude)
                }
                _ => println!("{bssid}: not found"),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_bssids() {
        assert!(bssid_parser("00:11:22:33:44:55").is_ok());
        assert!(bssid_parser("aa-bb-cc-dd-ee-ff").is_ok());
        assert!(bssid_parser("A1:B2:C3:D4:E5:F6").is_ok());
    }

    #[test]
    fn rejects_invalid_bssids() {
        assert!(bssid_parser("zz:11:22:33:44:55").is_err());
        assert!(bssid_parser("00:11:22:33:44").is_err());
        assert!(bssid_parser("001122334455").is_err());
        assert!(bssid_parser("").is_err());
    }
}
