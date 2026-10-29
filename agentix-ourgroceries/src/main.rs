use agentix_assistant_core::{Error, Groceries, Result};
use agentix_ourgroceries::Client;

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("OurGroceries request failed; check runtime credentials, connectivity and selected list.");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !matches!(args.as_slice(), [command] if command == "lists")
        && !matches!(args.as_slice(), [command, _] if command == "show")
    {
        eprintln!("Usage: agentix-ourgroceries lists | show LIST_ID\nCredentials: OURGROCERIES_EMAIL and OURGROCERIES_PASSWORD (runtime environment).");
        return Err(Error::Groceries);
    }
    let email = std::env::var("OURGROCERIES_EMAIL").map_err(|_| Error::Groceries)?;
    let password = std::env::var("OURGROCERIES_PASSWORD").map_err(|_| Error::Groceries)?;
    let client = Client::new(email, password, args.get(1).cloned())?;
    let json = if args[0] == "lists" {
        serde_json::to_string_pretty(&client.lists().await?)
    } else {
        serde_json::to_string_pretty(&client.snapshot().await?)
    }
    .map_err(|_| Error::Groceries)?;
    println!("{json}");
    Ok(())
}
