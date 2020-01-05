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

    let api = API::new(api_key, api_secret);
    let auth = api.start_auth().await?;
    println!("auth_url: {}", auth.url);

    for _ in 0..5 {
        api.check_auth(&auth).await?;
        delay_for(Duration::from_millis(3000)).await;
    }

    Ok(())
}