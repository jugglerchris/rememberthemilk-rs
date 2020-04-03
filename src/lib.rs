#![deny(warnings)]
//#![deny(missing_docs)]
//! Interface to the [remember the milk](https://www.rememberthemilk.com/) to-do
//! app via the [REST API](https://www.rememberthemilk.com/services/api/).
//!
//! This crate is unofficial and not not supported by remember the milk.  To use
//! it, you will need a free for non-commercial use [API
//! key](https://www.rememberthemilk.com/services/api/), which is not included
//! with the crate.
//!
//! Before doing anything else, you need to get an [API] object which needs your
//! API key and secret, and authenticate with the API - this means both your
//! application key and the user's account.
//!
//! ```rust
//! // Create the API object
//! let rtm_api = API::new("my key", "my secret");
//! // Begin authentication using your API key
//! let auth = rtm_api.start_auth().await?;
//! // auth.url is a URL which the user should visit to authorise the application
//! // using their rememberthemilk.com account.  The user needs to visit this URL
//! // and sign in before continuing below.
//! if api.check_auth(&auth).await? {
//!    // Successful authentication!  Can continue to use the API now.
//! }
//! ```
//!
//! If the authentication is successful, the [API](API) object will have an
//! authentication token which can be re-used later.  See [to_config](API::to_config)
//! and [from_config](API::from_config) which can be used to save the token and
//! API keys (so they should be stored somewhere relatively secure).
//!
//! The rest of the API can then be used:
//!
//! ``rust
//! # let api: API = unimplemented!();
//! let tasks = api.get_all_tasks().await?;
//! for list in all_tasks.list {
//!    if let Some(v) = list.taskseries {
//!        for ts in v {
//!            println!("  {}", ts.name);
//!        }
//!    }
//! }
//! ```
use chrono::{DateTime, Utc};
use failure::{bail, Error};
use md5;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::from_str;

static MILK_REST_URL: &'static str = "https://api.rememberthemilk.com/services/rest/";
static MILK_AUTH_URL: &'static str = "https://www.rememberthemilk.com/services/auth/";

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename = "err")]
/// Error type for Remember the Milk API calls.
pub struct RTMError {
    code: isize,
    msg: String,
}

#[derive(Serialize, Deserialize, Default)]
/// rememberthemilk API and authentication configuration.
/// This holds the persistent state for the app authentication
/// and possibly user authentication.
pub struct RTMConfig {
    /// The rememberthemilk API key.  See [RTM API](https://www.rememberthemilk.com/services/api/)
    /// to request an API key and secret.
    pub api_key: Option<String>,
    /// The rememberthemilk API secret.  See [RTM API](https://www.rememberthemilk.com/services/api/)
    /// to request an API key and secret.
    pub api_secret: Option<String>,
    /// A user authentication token retrieved from rememberthemilk.  This can be `None` but the user
    /// will need to authenticate before using the API.
    pub token: Option<String>,
    /// Details of the currently authenticated user.
    pub user: Option<User>,
}

/// The rememberthemilk API object.  All rememberthemilk operations are done using methods on here.
pub struct API {
    api_key: String,
    api_secret: String,
    token: Option<String>,
    user: Option<User>,
}

#[derive(Deserialize, Debug, Serialize, Eq, PartialEq)]
struct FrobResponse {
    stat: Stat,
    frob: String,
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Perms {
    Read,
    Write,
    Delete,
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone)]
pub struct User {
    id: String,
    username: String,
    fullname: String,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename = "auth")]
struct Auth {
    token: String,
    perms: Perms,
    user: User,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Stat {
    Ok,
    Fail,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
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

use serde::de::IntoDeserializer;

// Thanks to https://github.com/serde-rs/serde/issues/1425#issuecomment-462282398
fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    let opt = Option::<String>::deserialize(de)?;
    let opt = opt.as_ref().map(String::as_str);
    match opt {
        None | Some("") => Ok(None),
        Some(s) => T::deserialize(s.into_deserializer()).map(Some),
    }
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(untagged)]
enum TagSer {
    List(Vec<()>),
    Tags { tag: Vec<String> },
}

fn deser_tags<'de, D>(de: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let res = TagSer::deserialize(de);
    match res {
        Err(e) => Err(e),
        Ok(TagSer::List(_)) => Ok(vec![]),
        Ok(TagSer::Tags { tag }) => Ok(tag),
    }
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct Task {
    pub id: String,
    #[serde(deserialize_with = "empty_string_as_none")]
    pub due: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct TaskSeries {
    pub id: String,
    pub name: String,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub task: Vec<Task>,
    #[serde(deserialize_with = "deser_tags")]
    pub tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct RTMTasks {
    pub rev: String,
    #[serde(default)]
    pub list: Vec<RTMLists>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct RTMLists {
    pub id: String,
    pub taskseries: Option<Vec<TaskSeries>>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct TasksResponse {
    stat: Stat,
    tasks: RTMTasks,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename = "list")]
pub struct RTMList {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct ListContainer {
    list: Vec<RTMList>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct ListsResponse {
    stat: Stat,
    lists: ListContainer,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct Transaction {
    id: String,
    undoable: String,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct AddTagResponse {
    stat: Stat,
    transaction: Transaction,
    list: RTMLists,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct RTMResponse<T> {
    rsp: T,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct TimelineResponse {
    stat: Stat,
    timeline: String,
}

pub struct RTMTimeline(String);

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

    fn sign_keys(&self, keys: &[(String, String)]) -> String {
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

    async fn make_authenticated_request(
        &self,
        url: &'static str,
        keys: Vec<(String, String)>,
    ) -> Result<String, failure::Error> {
        let url = self.make_authenticated_url(url, keys);
        let body = reqwest::get(&url).await?.text().await?;
        //println!("Body={}", body);
        Ok(body)
    }

    async fn get_frob(&self) -> Result<String, Error> {
        let response = self
            .make_authenticated_request(
                MILK_REST_URL,
                vec![
                    ("method".into(), "rtm.auth.getFrob".into()),
                    ("format".into(), "json".into()),
                    ("api_key".into(), self.api_key.clone()),
                ],
            )
            .await?;
        let frob_resp = from_str::<RTMResponse<FrobResponse>>(&response)
            .unwrap()
            .rsp;
        Ok(frob_resp.frob)
    }

    pub async fn start_auth(&self) -> Result<AuthState, Error> {
        let frob = self.get_frob().await?;
        let url = self.make_authenticated_url(
            MILK_AUTH_URL,
            vec![
                ("api_key".into(), self.api_key.clone()),
                ("format".into(), "json".into()),
                ("perms".into(), "write".into()),
                ("frob".into(), frob.clone()),
            ],
        );
        Ok(AuthState { frob, url })
    }

    pub async fn check_auth(&mut self, auth: &AuthState) -> Result<bool, Error> {
        let response = self
            .make_authenticated_request(
                MILK_REST_URL,
                vec![
                    ("method".into(), "rtm.auth.getToken".into()),
                    ("format".into(), "json".into()),
                    ("api_key".into(), self.api_key.clone()),
                    ("frob".into(), auth.frob.clone()),
                ],
            )
            .await?;

        //println!("{:?}", response);
        let auth_rep = from_str::<RTMResponse<AuthResponse>>(&response)
            .unwrap()
            .rsp;
        self.token = Some(auth_rep.auth.token);
        self.user = Some(auth_rep.auth.user);
        Ok(true)
    }

    pub async fn has_token(&self) -> Result<bool, Error> {
        if let Some(ref tok) = self.token {
            let response = self
                .make_authenticated_request(
                    MILK_REST_URL,
                    vec![
                        ("method".into(), "rtm.auth.checkToken".into()),
                        ("format".into(), "json".into()),
                        ("api_key".into(), self.api_key.clone()),
                        ("auth_token".into(), tok.clone()),
                    ],
                )
                .await?;
            // We don't need to look inside the response as long as we receive one without
            // error.
            let _ar = from_str::<RTMResponse<AuthResponse>>(&response)?.rsp;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn get_all_tasks(&self) -> Result<RTMTasks, Error> {
        self.get_tasks_filtered("").await
    }
    pub async fn get_tasks_filtered(&self, filter: &str) -> Result<RTMTasks, Error> {
        if let Some(ref tok) = self.token {
            let mut params = vec![
                ("method".into(), "rtm.tasks.getList".into()),
                ("format".into(), "json".into()),
                ("api_key".into(), self.api_key.clone()),
                ("auth_token".into(), tok.clone()),
            ];
            if filter != "" {
                params.push(("filter".into(), filter.into()));
            }
            let response = self
                .make_authenticated_request(MILK_REST_URL, params)
                .await?;
            //println!("Got response:\n{}", response);
            // TODO: handle failure
            let tasklist = from_str::<RTMResponse<TasksResponse>>(&response)
                .unwrap()
                .rsp
                .tasks;
            Ok(tasklist)
        } else {
            bail!("Unable to fetch tasks")
        }
    }
    pub async fn get_lists(&self) -> Result<Vec<RTMList>, Error> {
        if let Some(ref tok) = self.token {
            let params = vec![
                ("method".into(), "rtm.lists.getList".into()),
                ("format".into(), "json".into()),
                ("api_key".into(), self.api_key.clone()),
                ("auth_token".into(), tok.clone()),
            ];
            let response = self
                .make_authenticated_request(MILK_REST_URL, params)
                .await?;
            //println!("Got response:\n{}", response);
            // TODO: handle failure
            let lists = from_str::<RTMResponse<ListsResponse>>(&response)
                .unwrap()
                .rsp
                .lists;
            Ok(lists.list)
        } else {
            bail!("Unable to fetch tasks")
        }
    }
    pub async fn get_timeline(&self) -> Result<RTMTimeline, Error> {
        if let Some(ref tok) = self.token {
            let params = vec![
                ("method".into(), "rtm.timelines.create".into()),
                ("format".into(), "json".into()),
                ("api_key".into(), self.api_key.clone()),
                ("auth_token".into(), tok.clone()),
            ];
            let response = self
                .make_authenticated_request(MILK_REST_URL, params)
                .await?;
            //println!("Got response:\n{}", response);
            // TODO: handle failure
            let tl = from_str::<RTMResponse<TimelineResponse>>(&response)
                .unwrap()
                .rsp
                .timeline;
            Ok(RTMTimeline(tl))
        } else {
            bail!("Unable to fetch tasks")
        }
    }

    pub async fn add_tag(
        &self,
        timeline: &RTMTimeline,
        list: &RTMLists,
        taskseries: &TaskSeries,
        task: &Task,
        tags: &[&str],
    ) -> Result<(), Error> {
        if let Some(ref tok) = self.token {
            let params = vec![
                ("method".into(), "rtm.tasks.addTags".into()),
                ("format".into(), "json".into()),
                ("api_key".into(), self.api_key.clone()),
                ("auth_token".into(), tok.clone()),
                ("timeline".into(), timeline.0.clone()),
                ("list_id".into(), list.id.clone()),
                ("taskseries_id".into(), taskseries.id.clone()),
                ("task_id".into(), task.id.clone()),
                ("tags".into(), tags.join(",")),
            ];
            let response = self
                .make_authenticated_request(MILK_REST_URL, params)
                .await?;
            let rsp = from_str::<RTMResponse<AddTagResponse>>(&response)?.rsp;
            if let Stat::Ok = rsp.stat {
                Ok(())
            } else {
                bail!("Error adding task")
            }
        } else {
            bail!("Unable to fetch tasks")
        }
    }
}

#[cfg(test)]
mod tests;
