use md5;
use failure::{Fail,Error};
use reqwest;

static MILK_REST_URL: &'static str = "https://api.rememberthemilk.com/services/rest/";

#[derive(Debug,Fail)]
pub enum MilkError {
    #[fail(display = "HTTP error")]
    HTTPError(#[cause] reqwest::Error),
}

pub struct API {
    api_key: String,
    api_secret: String,
}

impl API {
    pub fn new(api_key: String, api_secret: String) -> API {
        API {
            api_key,
            api_secret,
        }
    }

    fn sign_keys(&self, keys: &[(String, String)]) -> Result<String, Error>
    {
        let mut my_keys = keys.iter().collect::<Vec<&(String, String)>>();
        my_keys.sort();
        let mut to_sign = self.api_secret.clone();
        for &(ref k, ref v) in my_keys {
            to_sign += k;
            to_sign += v;
        }
        let digest = md5::compute(to_sign.as_bytes());
        Ok(format!("{:x}", digest))
    }

    async fn make_authenticated_request(&self, url: &'static str, keys: Vec<(String, String)>) -> Result<String, failure::Error> {
        let mut url = url.to_string();
        let auth_string = self.sign_keys(&keys)?;
        url.push('?');
        for (k, v) in keys {
            // Todo: URL: encode - maybe reqwest can help?
            url += &k;
            url.push('=');
            url += &v;
            url.push('&');
        }
        url += "api_sig=";
        url += &auth_string;

        let body = reqwest::get(&url)
                    .await?
                    .text()
                    .await?;
        println!("Body={}", body);
        Ok(body)
    }

    pub async fn get_frob(&self) -> Result<String, Error> {
        self.make_authenticated_request(MILK_REST_URL, vec![
            ("method".into(), "rtm.auth.getFrob".into()),
            ("api_key".into(), self.api_key.clone())
        ]).await
        
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
