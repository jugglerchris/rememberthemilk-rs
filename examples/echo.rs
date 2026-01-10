use std::env;

#[tokio::main]
async fn main() -> Result<(), reqwest::Error> {
    let api_key = env::args().nth(1).unwrap();
    let body = reqwest::get(&format!(
        "https://api.rememberthemilk.com/services/rest/?method=rtm.test.echo&api_key={}&name=foo",
        api_key
    ))
    .await?
    .text()
    .await?;
    println!("Body={}", body);
    Ok(())
}
