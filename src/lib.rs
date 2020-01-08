use md5;
use failure::{Fail,Error};
use reqwest;
use serde_json::{from_str, to_string};
use serde::{Deserialize, Serialize};

static MILK_REST_URL: &'static str = "https://api.rememberthemilk.com/services/rest/";
static MILK_AUTH_URL: &'static str = "https://www.rememberthemilk.com/services/auth/";

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename = "err")]
pub struct RTMError {
    code: isize,
    msg: String,
}

#[derive(Debug,Fail)]
pub enum MilkError {
    #[fail(display = "HTTP error")]
    HTTPError(#[cause] reqwest::Error),
}

#[derive(Serialize, Deserialize, Default)]
pub struct RTMConfig {
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub token: Option<String>,
    pub user: Option<User>,
}

pub struct API {
    api_key: String,
    api_secret: String,
    token: Option<String>,
    user: Option<User>,
}

#[derive(Deserialize, Debug)]
#[serde(rename="rsp")]
struct FrobResponse {
    stat: String,
    frob: String,
}

#[derive(Deserialize, Serialize, Debug,Eq, PartialEq)]
#[serde(rename_all="lowercase")]
enum Perms {
    Read,
    Write,
    Delete,
}

#[derive(Deserialize, Serialize, Debug,Eq, PartialEq, Clone)]
pub struct User {
    id: String,
    username: String,
    fullname: String,
}

#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
#[serde(rename = "auth")]
struct Auth {
    token: String,
    perms: Perms,
    user: User,
}


#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
struct AuthResponse {
    stat: String,
    auth: Auth,
}

#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
struct RTMResponse<T> {
    rsp: T,
}

pub struct AuthState {
    frob: String,
    pub url: String,
}

impl API {
    pub fn new(api_key: String, api_secret: String) -> API {
        API {
            api_key,
            api_secret,
            token: None,
            user: None,
        }
    }

    pub fn from_config(config: RTMConfig) -> API {
        API {
            api_key: config.api_key.unwrap(),
            api_secret: config.api_secret.unwrap(),
            token: config.token,
            user: config.user,
        }
    }

    pub fn to_config(&self) -> RTMConfig {
        RTMConfig {
            api_key: Some(self.api_key.clone()),
            api_secret: Some(self.api_secret.clone()),
            token: self.token.clone(),
            user: self.user.clone(),
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
            ("format".into(), "json".into()),
            ("api_key".into(), self.api_key.clone())
        ]).await?;
        let frob: FrobResponse = from_str(&response).unwrap();
        Ok(frob.frob)
    }

    pub async fn start_auth(&self) -> Result<AuthState, Error> {
        let frob = self.get_frob().await?;
        let url = self.make_authenticated_url(MILK_AUTH_URL, vec![
            ("api_key".into(), self.api_key.clone()),
            ("format".into(), "json".into()),
            ("perms".into(), "read".into()),
            ("frob".into(), frob.clone())
        ]);
        Ok(AuthState { frob, url })
    }

    pub async fn check_auth(&mut self, auth: &AuthState) -> Result<bool, Error> {
        let response = self.make_authenticated_request(MILK_REST_URL, vec![
            ("method".into(), "rtm.auth.getToken".into()),
            ("format".into(), "json".into()),
            ("api_key".into(), self.api_key.clone()),
            ("frob".into(), auth.frob.clone()),
        ]).await?;

        println!("{:?}", response);
        let auth_rep: AuthResponse = from_str(&response).unwrap();
        self.token = Some(auth_rep.auth.token);
        self.user = Some(auth_rep.auth.user);
        Ok(true)
    }

    pub async fn has_token(&self) -> Result<bool, Error> {
        if let Some(ref tok) = self.token {
            let response = self.make_authenticated_request(MILK_REST_URL, vec![
                ("method".into(), "rtm.auth.checkToken".into()),
                ("format".into(), "json".into()),
                ("api_key".into(), self.api_key.clone()),
                ("auth_token".into(), tok.clone()),
            ]).await?;
            // TODO: handle failure
            let auth_rep: AuthResponse = from_str(&response).unwrap();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn get_all_tasks(&self) -> Result<(), Error> {
        if let Some(ref tok) = self.token {
            let response = self.make_authenticated_request(MILK_REST_URL, vec![
                ("method".into(), "rtm.tasks.getList".into()),
                ("format".into(), "json".into()),
                ("api_key".into(), self.api_key.clone()),
                ("auth_token".into(), tok.clone()),
            ]).await?;
            println!("Got response:\n{}", response);
        } else {
            println!("No token");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;
    use super::*;
    /*
    #[test]
    fn deser_auth_response() {
        let ar: RTMResponse<Auth> =  from_str(r#"<rsp stat="ok">
          <token>asdf</token>
          <perms>read</perms>
          <user id="2" username="adsf" fullname="laskjdaf" />
          <auth>
              <token>410c57262293e9d937ee5be75eb7b0128fd61b61</token>
              <perms>delete</perms>
              <user id="1" username="bob" fullname="Bob T. Monkey" />
          </auth>
      </rsp>"#).unwrap();
      assert_eq!(ar, RTMResponse::Ok(Auth {
              token: "410c57262293e9d937ee5be75eb7b0128fd61b61".into(),
              perms: Perms::Delete,
              user: User {
                  id: 1,
                  username: "bob".into(),
                  fullname: "Bob T. Monkey".into(),
              }
      }));
    }

    #[test]
    fn deser_auth_response2() {
        let ar: AuthResponse =  from_str(r#"<rsp stat="ok">
          <auth>
              <token>410c57262293e9d937ee5be75eb7b0128fd61b61</token>
              <perms>delete</perms>
              <user id="1" username="bob" fullname="Bob T. Monkey" />
          </auth>
      </rsp>"#).unwrap();
      assert_eq!(ar, AuthResponse {
          stat: "ok".into(),
          auth: Auth {
              token: "410c57262293e9d937ee5be75eb7b0128fd61b61".into(),
              perms: Perms::Delete,
              user: User {
                  id: 1,
                  username: "bob".into(),
                  fullname: "Bob T. Monkey".into(),
              }
          },
      });
    }
    */

    #[test]
    fn deser_check_token()
    {
        let json_rsp = r#"{"rsp":{"stat":"ok","auth":{"token":"410c57262293e9d937ee5be75eb7b0128fd61b61","perms":"delete","user":{"id":"1","username":"bob","fullname":"Bob T. Monkey"}}}}"#;
        let expected = AuthResponse {
            stat: "ok".into(),
            auth: Auth {
                token: "410c57262293e9d937ee5be75eb7b0128fd61b61".into(),
                perms: Perms::Delete,
                user: User {
                    id: "1".into(),
                    username: "bob".into(),
                    fullname: "Bob T. Monkey".into(),
                }
            },
        };
        println!("{}", to_string(&expected).unwrap());
        println!("{}", json_rsp);
        let ar = from_str::<RTMResponse<AuthResponse>>(json_rsp).unwrap().rsp;
        assert_eq!(ar, expected);
    }
}
