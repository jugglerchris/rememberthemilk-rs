use md5;
use failure::{Fail,Error,bail};
use reqwest;
use serde_json::{from_str, to_string};
use chrono::{DateTime, Utc, TimeZone};
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
#[serde(rename_all = "lowercase")]
enum Stat {
    Ok,
    Fail,
}

#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
struct AuthResponse {
    stat: Stat,
    auth: Auth,
}

trait RTMToResult {
    type Type;
    fn into_result(self) -> Result<Self::Type, RTMError>;
}

impl RTMToResult for AuthResponse {
    type Type = Auth;
    fn into_result(self) -> Result<Auth, RTMError> {
        match self.stat {
            Stat::Ok => Ok(self.auth),
            Stat::Fail => panic!(),
        }
    }
}


#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
pub struct Task {
    id: String,
    due: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
pub struct TaskSeries {
    id: String,
    created: DateTime<Utc>,
    modified: DateTime<Utc>,
    task: Vec<Task>,
}

#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
struct RTMTasks {
    rev: String,
    list: Vec<TaskSeries>,
}

#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
struct TasksResponse {
    stat: Stat,
    tasks: RTMTasks,
}

#[derive(Serialize, Deserialize, Debug,Eq, PartialEq)]
struct RTMResponse<T> {
    rsp: T,
}

fn parse_response<'a, T: Deserialize<'a>+RTMToResult>(s: &'a str) -> Result<T::Type, RTMError> {
    let parsed = from_str::<RTMResponse<T>>(s).expect("Invalid response");
    parsed.rsp.into_result()
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
            let ar = from_str::<RTMResponse<AuthResponse>>(&response).unwrap().rsp;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn get_all_tasks(&self) -> Result<TaskSeries, Error> {
        if let Some(ref tok) = self.token {
            let response = self.make_authenticated_request(MILK_REST_URL, vec![
                ("method".into(), "rtm.tasks.getList".into()),
                ("format".into(), "json".into()),
                ("api_key".into(), self.api_key.clone()),
                ("auth_token".into(), tok.clone()),
            ]).await?;
            //println!("Got response:\n{}", response);
            // TODO: handle failure
            let tasklist = from_str::<RTMResponse<TaskSeries>>(&response).unwrap().rsp;
            Ok(tasklist)
        } else {
            bail!("Unable to fetch tasks")
        }
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
            stat: Stat::Ok,
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

    #[test]
    fn test_deser_taskseries()
    {
        let json = r#"
               {"id":"blahid",
                "created":"2020-01-01T16:00:00Z",
                "modified":"2020-01-02T13:12:15Z",
                "name":"Do the thing",
                "source":"android",
                "url":"",
                "location_id":"",
                "tags":{"tag":["computer"]},
                "participants":[],
                "notes":[],
                "task":[
                  {"id":"my_task_id","due":"2020-01-12T00:00:00Z","has_due_time":"0","added":"2020-01-10T16:00:56Z","completed":"2020-01-12T13:12:11Z","deleted":"","priority":"N","postponed":"0","estimate":""}
                ]
               }"#;
//        println!("{}", json);
        let expected = TaskSeries {
            id: "blahid".into(),
            created: chrono::Utc.ymd(2020, 1, 1).and_hms(16, 0, 0),
            modified: chrono::Utc.ymd(2020, 1, 2).and_hms(13, 12, 15),
            task: vec![
                Task {
                    id: "my_task_id".into(),
                    due: chrono::Utc.ymd(2020, 1, 12).and_hms(0, 0, 0),
                },
            ],
        };
        println!("{}", to_string(&expected).unwrap());
        let tasks = from_str::<TaskSeries>(json).unwrap();
        assert_eq!(tasks, expected);
    }

    #[test]
    fn test_deser_task()
    {
        let json = r#"
                  {"id":"my_task_id","due":"2020-01-12T00:00:00Z","has_due_time":"0","added":"2020-01-10T16:00:56Z","completed":"2020-01-12T13:12:11Z","deleted":"","priority":"N","postponed":"0","estimate":""}
"#;
//        println!("{}", json);
        let expected = Task {
            id: "my_task_id".into(),
            due: chrono::Utc.ymd(2020, 1, 12).and_hms(0, 0, 0),
        };
        println!("{}", to_string(&expected).unwrap());
        let task = from_str::<Task>(json).unwrap();
        assert_eq!(task, expected);
    }

    #[test]
    fn test_deser_tasklist_response()
    {
        let json = r#"{"rsp": { "stat": "ok",
               "tasks": {"rev": "my_rev",
                         "list": []}}}"#;
//        println!("{}", json);
        let expected = TasksResponse {
            stat: Stat::Ok,
            tasks: RTMTasks {
                rev: "my_rev".into(),
                list: vec![],
            },
        };
        println!("{}", to_string(&expected).unwrap());
        let lists = from_str::<RTMResponse<TasksResponse>>(json).unwrap().rsp;
        assert_eq!(lists, expected);
    }
}
