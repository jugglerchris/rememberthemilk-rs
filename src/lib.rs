use md5;
use failure::{Fail,Error};
use reqwest;
use serde_xml_rs::from_str;
use serde::Deserialize;

static MILK_REST_URL: &'static str = "https://api.rememberthemilk.com/services/rest/";
static MILK_AUTH_URL: &'static str = "https://www.rememberthemilk.com/services/auth/";

#[derive(Debug,Fail)]
pub enum MilkError {
    #[fail(display = "HTTP error")]
    HTTPError(#[cause] reqwest::Error),
}

pub struct API {
    api_key: String,
    api_secret: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename="rsp")]
struct FrobResponse {
    stat: String,
    frob: String,
}

impl API {
    pub fn new(api_key: String, api_secret: String) -> API {
        API {
            api_key,
            api_secret,
        }
    }

    fn sign_keys(&self, keys: &[(String, String)]) -> String
    {
        let mut my_keys = keys.iter().collect::<Vec<&(String, String)>>();
        my_keys.sort();
        let mut to_sign = self.api_secret.clone();
        for &(ref k, ref v) in my_keys {
            to_sign += k;
            to_sign += v;
        }
        let digest = md5::compute(to_sign.as_bytes());
        format!("{:x}", digest)
    }

    fn make_authenticated_url(&self, url: &'static str, keys: Vec<(String, String)>) -> String {
        let mut url = url.to_string();
        let auth_string = self.sign_keys(&keys);
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
        url
    }

    async fn make_authenticated_request(&self, url: &'static str, keys: Vec<(String, String)>) -> Result<String, failure::Error> {
        let url = self.make_authenticated_url(url, keys);
        let body = reqwest::get(&url)
                    .await?
                    .text()
                    .await?;
        println!("Body={}", body);
        Ok(body)
    }

    async fn get_frob(&self) -> Result<String, Error> {
        let response = self.make_authenticated_request(MILK_REST_URL, vec![
            ("method".into(), "rtm.auth.getFrob".into()),
            ("api_key".into(), self.api_key.clone())
        ]).await?;
        let frob: FrobResponse = from_str(&response).unwrap();
        Ok(frob.frob)
    }

    pub async fn get_auth_url(&self) -> Result<String, Error> {
        let frob = self.get_frob().await?;
        let url = self.make_authenticated_url(MILK_AUTH_URL, vec![
            ("api_key".into(), self.api_key.clone()),
            ("perms".into(), "read".into()),
            ("frob".into(), frob)
        ]);
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
