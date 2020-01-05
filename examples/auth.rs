use std::env;
use rememberthemilk::API;

#[tokio::main]
async fn main() -> Result<(), failure::Error>
{
    let args: Vec<String> = env::args().collect();
    let api_key = args[1].clone();
    let api_secret = args[2].clone();

    let api = API::new(api_key, api_secret);
    let frob = api.get_frob().await?;
    println!("frob={}", frob);
    Ok(())
}