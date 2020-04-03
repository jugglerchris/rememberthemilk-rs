#![deny(warnings)]
#![deny(missing_docs)]
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
//! ```no_run
//! # #[tokio::main]
//! # async fn main() -> Result<(), failure::Error> {
//! // Create the API object
//! # use rememberthemilk::API;
//! let mut rtm_api = API::new("my key".to_string(), "my secret".to_string());
//! // Begin authentication using your API key
//! let auth = rtm_api.start_auth().await?;
//! // auth.url is a URL which the user should visit to authorise the application
//! // using their rememberthemilk.com account.  The user needs to visit this URL
//! // and sign in before continuing below.
//! if rtm_api.check_auth(&auth).await? {
//!    // Successful authentication!  Can continue to use the API now.
//! }
//! # Ok(())
//! # }
//! ```
//!
//! If the authentication is successful, the [API](API) object will have an
//! authentication token which can be re-used later.  See [to_config](API::to_config)
//! and [from_config](API::from_config) which can be used to save the token and
//! API keys (so they should be stored somewhere relatively secure).
//!
//! The rest of the API can then be used:
//!
//! ```no_run
//! # #[tokio::main]
//! # async fn main() -> Result<(), failure::Error> {
//! # use rememberthemilk::API;
//! # let api: API = unimplemented!();
//! let tasks = api.get_all_tasks().await?;
//! for list in tasks.list {
//!    if let Some(v) = list.taskseries {
//!        for ts in v {
//!            println!("  {}", ts.name);
//!        }
//!    }
//! }
//! # Ok(())
//! # }
//! ```
use chrono::{DateTime, Utc};
use failure::{bail, Error};
use md5;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::from_str;

static MILK_REST_URL: &str = "https://api.rememberthemilk.com/services/rest/";
static MILK_AUTH_URL: &str = "https://www.rememberthemilk.com/services/auth/";

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
/// Information about a rememberthemilk user.
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
    let opt = opt.as_deref();
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
/// A rememberthemilk Task Series.  This corresponds to a single to-do item,
/// and has the fields such as name and tags.  It also may contain some
/// [Task]s, each of which is an instance of a possibly recurring or
/// repeating task.
pub struct TaskSeries {
    /// The task series' unique id within its list.
    pub id: String,
    /// The name of the task.
    pub name: String,
    /// The creation time.
    pub created: DateTime<Utc>,
    /// The last modification time.
    pub modified: DateTime<Utc>,
    /// The tasks within this series, if any.
    pub task: Vec<Task>,
    #[serde(deserialize_with = "deser_tags")]
    /// A list of the tags attached to this task series.
    pub tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
/// A rememberthemilk Task.  In rememberthemilk a task is
/// a specific instance of a possibly repeating item.  For
/// example, a weekly task to take out the bins is
/// represented as a single [TaskSeries] with a different
/// [Task] every week.  A Task's main characteristic is a
/// due date.
pub struct Task {
    /// The task's unique (within the list and task series) id.
    pub id: String,
    #[serde(deserialize_with = "empty_string_as_none")]
    /// The task's due date, if any.
    pub due: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
/// The response from fetching a list of tasks.
pub struct RTMTasks {
    rev: String,
    #[serde(default)]
    /// The list of tasks.
    pub list: Vec<RTMLists>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
/// A container for a list of task series.
pub struct RTMLists {
    /// The unique id for this list of tasks series.
    pub id: String,
    /// The task series themselves.
    pub taskseries: Option<Vec<TaskSeries>>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct TasksResponse {
    stat: Stat,
    tasks: RTMTasks,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename = "list")]
/// The details of a list of to-do items.
pub struct RTMList {
    /// The list's unique ID.
    pub id: String,
    /// The name of this list.
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

/// Handle to a rememberthemilk timeline.
///
/// This is required for API calls which can modify state.  They can also
/// be used to undo (within a timeline) but this is not yet implemented.
pub struct RTMTimeline(String);

/// The state of an ongoing user authentication attempt.
pub struct AuthState {
    frob: String,
    /// The URL to which the user should be sent.  They will be asked
    /// to log in to rememberthemilk and allow the application access.
    pub url: String,
}

impl API {
    /// Create a new rememberthemilk API instance, with no user associated.
    ///
    /// A user will need to authenticate; see [API::start_auth].
    ///
    /// The `api_key` and `api_secret` are for authenticating the application.
    /// They can be [requested from rememberthemilk](https://www.rememberthemilk.com/services/api/).
    pub fn new(api_key: String, api_secret: String) -> API {
        API {
            api_key,
            api_secret,
            token: None,
            user: None,
        }
    }

    /// Create a new rememberthemilk API instance from saved configuration.
    ///
    /// The configuration may or may not include a valid user authentication
    /// token.  If not, then the next step is callnig [API::start_auth].
    ///
    /// The `config` will usually be generated from a previous session, where
    /// [API::to_config] was used to save the session state.
    pub fn from_config(config: RTMConfig) -> API {
        API {
            api_key: config.api_key.unwrap(),
            api_secret: config.api_secret.unwrap(),
            token: config.token,
            user: config.user,
        }
    }

    /// Extract a copy of the rememberthemilk API state.
    ///
    /// If a user has been authenticated in this session (or a previous one
    /// one and restored) then this will include a user authentication token
    /// as well as the API key and secret.  This can be serialised and used
    /// next time avoiding having to go through the authentication procedure
    /// every time.
    ///
    /// Note that this contains app and user secrets, so should not be stored
    /// anywhere where other users may be able to access.
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

    /// Begin user authentication.
    ///
    /// If this call is successful (which requires a valid API key and secret,
    /// and a successful interaction with the rememberthemilk API) then the
    /// returned [AuthState] contains a URL which a user should open (e.g. by
    /// a web view or separate web browser instance, redirect, etc.  depending
    /// on the type of application).
    ///
    /// After the user has logged in and authorised the application, you can
    /// use [API::check_auth] to verify that this was successful.
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

    /// Check whether a user authentication attempt was successful.
    ///
    /// This should be called after the user has had a chance to visit the URL
    /// returned by [API::start_auth].  It can be called multiple times to poll.
    ///
    /// If authentication has been successful then a user auth token will be
    /// available (and retrievable using [API::to_config]) and true will be
    /// returned.  Other API calls can be made.
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

    /// Check whether we have a valid user token.
    ///
    /// Returns true if so, false if none, and an error if the token
    /// is not valid (e.g.  expired).  [API::start_auth] will be needed if
    /// not successful to re-authenticate the user.
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

    /// Retrieve a list of all tasks.
    ///
    /// This may be a lot of tasks if the user has been using rememberthemilk
    /// for some time, and is usually not needed unless exporting or backing
    /// up the whole thing.
    ///
    /// Requires a valid user authentication token.
    pub async fn get_all_tasks(&self) -> Result<RTMTasks, Error> {
        self.get_tasks_filtered("").await
    }

    /// Retrieve a filtered list of tasks.
    ///
    /// The `filter` is a string in the [format used by
    /// rememberthemilk](https://www.rememberthemilk.com/help/?ctx=basics.search.advanced),
    /// for example to retrieve tasks which have not yet been completed and
    /// are due today or in the past, you could use:
    ///
    /// `"status:incomplete AND (dueBefore:today OR due:today)"`
    ///
    /// Requires a valid user authentication token.
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
    /// Request a list of rememberthemilk lists.
    ///
    /// Requires a valid user authentication token.
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
    /// Request a fresh remember timeline.
    ///
    /// A timeline is required for any request which modifies data on the
    /// server.
    ///
    /// Requires a valid user authentication token.
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

    /// Add one or more tags to a task.
    ///
    /// * `timeline`: a timeline as retrieved using [API::get_timeline]
    /// * `list`, `taskseries` and `task` identify the task to tag.
    /// * `tags` is a slice of tags to add to this task.
    ///
    /// Requires a valid user authentication token.
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
