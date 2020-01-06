use std::env;
use rememberthemilk::API;
use tokio::time::delay_for;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), failure::Error>
{
    let args: Vec<String> = env::args().collect();
    let api_key = args[1].clone();
    let api_secret = args[2].clone();

    let mut api = API::new(api_key, api_secret);
    let auth = api.start_auth().await?;
    println!("auth_url: {}", auth.url);
    println!("Press enter when authorised...");
    {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        let mut lines = stdin.lock().lines();
        lines.next().unwrap().unwrap();
    }

    if api.check_auth(&auth).await? {
        println!("Successfull authorised");
    }

    Ok(())
}