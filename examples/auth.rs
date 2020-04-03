use confy;
use failure::bail;
use rememberthemilk::API;
use std::env;

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let config: rememberthemilk::RTMConfig = confy::load("rtm_auth_example")?;
    let mut api = if config.api_key.is_some() && config.api_secret.is_some() {
        let api = API::from_config(config);
        api
    } else {
        let args: Vec<String> = env::args().collect();
        let api_key = args[1].clone();
        let api_secret = args[2].clone();

        let api = API::new(api_key, api_secret);
        api
    };

    if !api.has_token().await.unwrap() {
        let auth = api.start_auth().await?;
        println!("auth_url: {}", auth.url);
        println!("Press enter when authorised...");
        {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            let mut lines = stdin.lock().lines();
            lines.next().unwrap().unwrap();
        }

        if !api.check_auth(&auth).await? {
            bail!("Error authenticating");
        }
        confy::store("rtm_auth_example", api.to_config())?;
    };
    println!("Getting all tasks...");
    println!("{:?}", api.get_all_tasks().await?);
    println!("Got all tasks.");

    Ok(())
}
