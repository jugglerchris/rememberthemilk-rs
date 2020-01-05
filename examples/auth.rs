use std::env;
use rememberthemilk::API;

#[tokio::main]
async fn main() -> Result<(), failure::Error>
{
    let args: Vec<String> = env::args().collect();
    let api_key = args[1].clone();
    let api_secret = args[2].clone();

    let api = API::new(api_key, api_secret);
    let url = api.get_auth_url().await?;
    println!("auth_url: {}", url);
    Ok(())
}