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
//! # use rememberthemilk::{API, Perms};
//! let mut rtm_api = API::new("my key".to_string(), "my secret".to_string());
//! // Begin authentication using your API key
//! let auth = rtm_api.start_auth(Perms::Read).await?;
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
use chrono::{DateTime, Duration, Utc};
use failure::{bail, Error};
use serde::{de::Unexpected, Deserialize, Serialize};
use serde_json::from_str;

#[cfg(test)]
fn get_auth_url() -> String {
    mockito::server_url()
}

#[cfg(not(test))]
fn get_auth_url() -> String {
    static MILK_AUTH_URL: &str = "https://www.rememberthemilk.com/services/auth/";
    MILK_AUTH_URL.to_string()
}

#[cfg(test)]
fn get_rest_url() -> String {
    mockito::server_url()
}

#[cfg(not(test))]
fn get_rest_url() -> String {
    static MILK_REST_URL: &str = "https://api.rememberthemilk.com/services/rest/";
    MILK_REST_URL.to_string()
}
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

impl RTMConfig {
    /// Clear any user-specific data (auth tokens, user info, etc.)
    pub fn clear_user_data(&mut self) {
        self.token = None;
        self.user = None;
    }
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

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Copy, Clone)]
#[serde(rename_all = "lowercase")]
/// rememberthemilk API permissions.
pub enum Perms {
    /// Permission to read the user's tasks and other data
    Read,
    /// Permission to modify the user's tasks and other data, but
    /// not to delete tasks.  This includes Read permission.
    Write,
    /// Permission to modify the user's tasks and other data, including
    /// deleting tasks.
    Delete,
}

impl Perms {
    /// Return true if this permission includes the rights to do `other`.
    fn includes(self, other: Perms) -> bool {
        match (self, other) {
            (Self::Delete, _)
            | (Self::Write, Self::Read)
            | (Self::Write, Self::Write)
            | (Self::Read, Self::Read) => true,
            _ => false,
        }
    }

    /// Return a string representation suitable for the RTM API
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Delete => "delete",
        }
    }
}

impl ToString for Perms {
    fn to_string(&self) -> String {
        self.as_str().to_string()
    }
}

impl std::str::FromStr for Perms {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "delete" => Ok(Self::Delete),
            _ => Err("Invalid perms string".into()),
        }
    }
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

fn bool_from_string<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match String::deserialize(deserializer)?.as_ref() {
        "0" => Ok(false),
        "1" => Ok(true),
        other => Err(serde::de::Error::invalid_value(
            Unexpected::Str(other),
            &"0 or 1",
        )),
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
/// A recurrence rule for a repeating task.
pub struct RRule {
    /// If true, the recurrence rule is an "every" rule, which means it
    /// continues repeating even if the task isn't completed.  Otherwise,
    /// it is an "after" task.
    #[serde(deserialize_with = "bool_from_string")]
    pub every: bool,

    /// The recurrence rule; see RFC 2445 for the meaning.
    #[serde(rename = "$t")]
    pub rule: String,
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
    /// Repetition information
    #[serde(rename = "rrule")]
    pub repeat: Option<RRule>,
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
    /// If true then there is a due date and time, not just date.
    #[serde(deserialize_with = "bool_from_string")]
    pub has_due_time: bool,
    #[serde(deserialize_with = "empty_string_as_none")]
    /// The task's deleted date, if any.
    pub deleted: Option<DateTime<Utc>>,
    #[serde(deserialize_with = "empty_string_as_none")]
    /// The date/time when this task was added
    pub added: Option<DateTime<Utc>>,
    #[serde(deserialize_with = "empty_string_as_none")]
    /// The date/time when this task was completed
    pub completed: Option<DateTime<Utc>>,
}

/// Describes how much time is left to complete this task, or perhaps
/// that it is overdue or has been deleted.
#[derive(Debug, Copy, Clone)]
pub enum TimeLeft {
    /// The length of time in seconds until this item is due (in the future)
    Remaining(u64),
    /// The task is overdue by this count of seconds
    Overdue(u64),
    /// Already completed
    Completed,
    /// No due date
    NoDue,
}

impl Task {
    /// Return the time left (or time since it was due) of a task.
    /// For tasks with no due date, or which are already completed,
    /// returns Completed.
    pub fn get_time_left(&self) -> TimeLeft {
        if self.completed.is_some() {
            return TimeLeft::Completed;
        }
        if self.deleted.is_some() {
            return TimeLeft::NoDue;
        }
        if self.due.is_none() || self.deleted.is_some() {
            return TimeLeft::NoDue;
        }
        if let Some(mut due) = self.due {
            if !self.has_due_time {
                // If no due time, assume it's due at the end of the day,
                // or the start of the next day.
                due = due + Duration::days(1);
            }
            let time_left = due.signed_duration_since(chrono::Utc::now());
            let seconds = time_left.num_seconds();
            if seconds < 0 {
                TimeLeft::Overdue((-seconds) as u64)
            } else {
                TimeLeft::Remaining(seconds as u64)
            }
        } else {
            // We would have found it in the previous test
            unreachable!()
        }
    }
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
    list: RTMLists,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
struct AddTaskResponse {
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

    fn sign_keys(&self, keys: &[(&str, &str)]) -> String {
        let mut my_keys = keys.iter().collect::<Vec<&(&str, &str)>>();
        my_keys.sort();
        let mut to_sign = self.api_secret.clone();
        for &(ref k, ref v) in my_keys {
            to_sign += k;
            to_sign += v;
        }
        let digest = md5::compute(to_sign.as_bytes());
        format!("{:x}", digest)
    }

    fn make_authenticated_url(&self, url: &str, keys: &[(&str, &str)]) -> String {
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

    fn make_authenticated_request<'a>(
        &'a self,
        url: &'a str,
        keys: &'a [(&'a str, &'a str)],
    ) -> impl std::future::Future<Output = Result<String, failure::Error>> + 'a {
        // As an async fn, this doesn't compile due to (I think):
        // https://github.com/rust-lang/rust/issues/63033
        // One of the comments points to an explicit async block instead of using
        // an async function as a workaround.
        let url = self.make_authenticated_url(url, keys);
        async move {
            let body = reqwest::get(&url).await?.text().await?;
            //println!("Body={}", body);
            Ok(body)
        }
    }

    async fn get_frob(&self) -> Result<String, Error> {
        let response = self
            .make_authenticated_request(
                &get_rest_url(),
                &[
                    ("method", "rtm.auth.getFrob"),
                    ("format", "json"),
                    ("api_key", &self.api_key),
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
    pub async fn start_auth(&self, perm: Perms) -> Result<AuthState, Error> {
        let frob = self.get_frob().await?;
        let url = self.make_authenticated_url(
            &get_auth_url(),
            &[
                ("api_key", &self.api_key),
                ("format", "json"),
                ("perms", perm.as_str()),
                ("frob", &frob),
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
                &get_rest_url(),
                &[
                    ("method", "rtm.auth.getToken"),
                    ("format", "json"),
                    ("api_key", &self.api_key),
                    ("frob", &auth.frob),
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

    /// Check whether we have a valid user token with the provided permission
    /// level.
    ///
    /// Returns true if so, false if none, and an error if the token
    /// is not valid (e.g.  expired).  [API::start_auth] will be needed if
    /// not successful to re-authenticate the user.
    pub async fn has_token(&self, perm: Perms) -> Result<bool, Error> {
        if let Some(ref tok) = self.token {
            let response = self
                .make_authenticated_request(
                    &get_rest_url(),
                    &[
                        ("method", "rtm.auth.checkToken"),
                        ("format", "json"),
                        ("api_key", &self.api_key),
                        ("auth_token", &tok),
                    ],
                )
                .await?;
            let ar = from_str::<RTMResponse<AuthResponse>>(&response)?.rsp;
            Ok(ar.auth.perms.includes(perm))
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
                ("method", "rtm.tasks.getList"),
                ("format", "json"),
                ("api_key", &self.api_key),
                ("auth_token", &tok),
                ("v", "2"),
            ];
            if filter != "" {
                params.push(("filter", filter));
            }
            let response = self
                .make_authenticated_request(&get_rest_url(), &params)
                .await?;
            eprintln!("Got response:\n{}", response);
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
            let params = &[
                ("method", "rtm.lists.getList"),
                ("format", "json"),
                ("api_key", &self.api_key),
                ("auth_token", &tok),
            ];
            let response = self
                .make_authenticated_request(&get_rest_url(), params)
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
            let params = &[
                ("method", "rtm.timelines.create"),
                ("format", "json"),
                ("api_key", &self.api_key),
                ("auth_token", &tok),
            ];
            let response = self
                .make_authenticated_request(&get_rest_url(), params)
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
            let tags = tags.join(",");
            let params = &[
                ("method", "rtm.tasks.addTags"),
                ("format", "json"),
                ("api_key", &self.api_key),
                ("auth_token", &tok),
                ("timeline", &timeline.0),
                ("list_id", &list.id),
                ("taskseries_id", &taskseries.id),
                ("task_id", &task.id),
                ("tags", &tags),
            ];
            let response = self
                .make_authenticated_request(&get_rest_url(), params)
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

    /// Add a new task
    ///
    /// * `timeline`: a timeline as retrieved using [API::get_timeline]
    /// * `name`: the new task's name
    /// * `list`: the optional list into which the task should go
    /// * `parent`: If specified, the parent task for the new task (pro accounts only)
    /// * `external_id`: An id which can be attached to this task.
    ///
    /// Requires a valid user authentication token.
    pub async fn add_task(
        &self,
        timeline: &RTMTimeline,
        name: &str,
        list: Option<&RTMLists>,
        parent: Option<&Task>,
        external_id: Option<&str>,
    ) -> Result<(), Error> {
        if let Some(ref tok) = self.token {
            let mut params = vec![
                ("method", "rtm.tasks.add"),
                ("format", "json"),
                ("api_key", &self.api_key),
                ("auth_token", &tok),
                ("timeline", &timeline.0),
                ("name", name),
            ];
            if let Some(list) = list {
                params.push(("list_id", &list.id));
            }
            if let Some(parent) = parent {
                params.push(("task_id", &parent.id));
            }
            if let Some(external_id) = external_id {
                params.push(("external_id", &external_id));
            }
            let response = self
                .make_authenticated_request(&get_rest_url(), &params)
                .await?;
            eprintln!("Add task response: {}", response);
            let rsp = from_str::<RTMResponse<AddTaskResponse>>(&response)?.rsp;
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
