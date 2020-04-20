use super::*;
use chrono::TimeZone;
#[cfg(test)]
use serde_json::{from_str, to_string};

#[test]
fn deser_check_token() {
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
            },
        },
    };
    println!("{}", to_string(&expected).unwrap());
    println!("{}", json_rsp);
    let ar = from_str::<RTMResponse<AuthResponse>>(json_rsp).unwrap().rsp;
    assert_eq!(ar, expected);
}

#[test]
fn test_deser_taskseries() {
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
            "rrule":{"every":"1","$t":"FREQ=WEEKLY;INTERVAL=1;WKST=MO"},
            "task":[
              {"id":"my_task_id","due":"2020-01-12T00:00:00Z","has_due_time":"0","added":"2020-01-10T16:00:56Z","completed":"2020-01-12T13:12:11Z","deleted":"","priority":"N","postponed":"0","estimate":""}
            ]
           }"#;
    //        println!("{}", json);
    let expected = TaskSeries {
        id: "blahid".into(),
        name: "Do the thing".into(),
        created: chrono::Utc.ymd(2020, 1, 1).and_hms(16, 0, 0),
        modified: chrono::Utc.ymd(2020, 1, 2).and_hms(13, 12, 15),
        repeat: Some(RRule {
            every: true,
            rule: "FREQ=WEEKLY;INTERVAL=1;WKST=MO".into(),
        }),
        task: vec![Task {
            id: "my_task_id".into(),
            due: Some(chrono::Utc.ymd(2020, 1, 12).and_hms(0, 0, 0)),
            added: Some(chrono::Utc.ymd(2020, 1, 10).and_hms(16, 0, 56)),
            completed: Some(chrono::Utc.ymd(2020, 1, 12).and_hms(13, 12, 11)),
            deleted: None,
            has_due_time: false,
        }],
        tags: vec!["computer".into()],
    };
    println!("{}", to_string(&expected).unwrap());
    let tasks = from_str::<TaskSeries>(json).unwrap();
    assert_eq!(tasks, expected);
}

#[test]
fn test_deser_rrule() {
    let json = r#"{"every":"1","$t":"FREQ=WEEKLY;INTERVAL=1;WKST=MO"}"#;
    let expected = RRule {
        every: true,
        rule: "FREQ=WEEKLY;INTERVAL=1;WKST=MO".into(),
    };
    println!("{}", to_string(&expected).unwrap());
    let rule = from_str::<RRule>(json).unwrap();
    assert_eq!(rule, expected);
}

#[test]
fn test_deser_task_nodue() {
    let json = r#"
              {"id":"my_task_id","due":"","has_due_time":"0","added":"2020-01-10T16:00:56Z","completed":"2020-01-12T13:12:11Z","deleted":"","priority":"N","postponed":"0","estimate":""}
"#;
    //        println!("{}", json);
    let expected = Task {
        id: "my_task_id".into(),
        due: None,
        added: Some(chrono::Utc.ymd(2020, 1, 10).and_hms(16, 0, 56)),
        completed: Some(chrono::Utc.ymd(2020, 1, 12).and_hms(13, 12, 11)),
        deleted: None,
        has_due_time: false,
    };
    println!("{}", to_string(&expected).unwrap());
    let task = from_str::<Task>(json).unwrap();
    assert_eq!(task, expected);
}

#[test]
fn test_deser_tag1() {
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
    let expected = vec!["computer".to_string()];
    println!("{}", to_string(&expected).unwrap());
    let tasks = from_str::<TaskSeries>(json).unwrap();
    assert_eq!(tasks.tags, expected);
}

#[test]
fn test_deser_tasklist_response() {
    let json = r#"{"rsp": { "stat": "ok",
           "tasks": {"rev": "my_rev",
                     "list": [
                       {"id": "my_list_id",
                        "taskseries": [
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
                             ]}
                         ]}
                     ]}}}"#;
    //        println!("{}", json);
    let expected = TasksResponse {
        stat: Stat::Ok,
        tasks: RTMTasks {
            rev: "my_rev".into(),
            list: vec![RTMLists {
                id: "my_list_id".into(),
                taskseries: Some(vec![TaskSeries {
                    id: "blahid".into(),
                    name: "Do the thing".into(),
                    created: chrono::Utc.ymd(2020, 1, 1).and_hms(16, 0, 0),
                    modified: chrono::Utc.ymd(2020, 1, 2).and_hms(13, 12, 15),
                    task: vec![Task {
                        id: "my_task_id".into(),
                        due: Some(chrono::Utc.ymd(2020, 1, 12).and_hms(0, 0, 0)),
                        added: Some(chrono::Utc.ymd(2020, 1, 10).and_hms(16, 0, 56)),
                        completed: Some(chrono::Utc.ymd(2020, 1, 12).and_hms(13, 12, 11)),
                        deleted: None,
                        has_due_time: false,
                    }],
                    tags: vec!["computer".into()],
                    repeat: None,
                }]),
            }],
        },
    };
    println!("{}", to_string(&expected).unwrap());
    let lists = from_str::<RTMResponse<TasksResponse>>(json).unwrap().rsp;
    assert_eq!(lists, expected);
}

#[tokio::test]
async fn test_no_token()
{
    use ::mockito::mock;

    let _m = mock("GET", "/");

    let api = API::new("key".into(), "secret".into());

    assert!(!api.has_token(Perms::Read).await.unwrap());
}
