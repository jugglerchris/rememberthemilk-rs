#![deny(warnings)]
use failure::bail;
use rememberthemilk::API;
use std::collections::HashMap;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
enum Command {
    /// Operate on tasks
    Tasks {
        #[structopt(long)]
        filter: Option<String>,
    },
    /// Show all lists
    Lists,
    /// Add a tag to filtered messages
    AddTag {
        tag: String,
        #[structopt(long)]
        filter: String,
    },
    /// Add a new task
    AddTask { name: String },
    /// Authorise the app
    AuthApp { key: String, secret: String },
    /// Remove the saved user token
    Logout,
}

#[derive(StructOpt, Debug)]
struct Opt {
    #[structopt(subcommand)]
    cmd: Command,
}

async fn get_rtm_api() -> Result<API, failure::Error> {
    let config: rememberthemilk::RTMConfig = confy::load("rtm_auth_example")?;
    let mut api = if config.api_key.is_some() && config.api_secret.is_some() {
        API::from_config(config)
    } else {
        eprintln!("Error, no API key saved.  Use `rtm auth-app` to supply them.");
        bail!("No auth key");
    };

    if !api.has_token().await.unwrap() {
        auth_user(&mut api).await?;
    };
    Ok(api)
}

async fn auth_user(api: &mut API) -> Result<(), failure::Error> {
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
    Ok(())
}

async fn auth_app(key: String, secret: String) -> Result<(), failure::Error> {
    let mut api = API::new(key, secret);

    auth_user(&mut api).await?;
    println!("Successfully authenticated.");
    Ok(())
}

async fn logout() -> Result<(), failure::Error> {
    let mut config: rememberthemilk::RTMConfig = confy::load("rtm_auth_example")?;
    config.clear_user_data();
    confy::store("rtm_auth_example", config)?;
    Ok(())
}

async fn list_tasks(filter: Option<String>) -> Result<(), failure::Error> {
    let api = get_rtm_api().await?;
    let filter = match filter {
        Some(ref s) => &s[..],
        None => "status:incomplete AND (dueBefore:today OR due:today)",
    };
    let all_tasks = api.get_tasks_filtered(filter).await?;
    let mut lists = HashMap::new();
    if !all_tasks.list.is_empty() {
        let all_lists = api.get_lists().await?;
        for list in all_lists {
            lists.insert(list.id.clone(), list);
        }
    }
    for list in all_tasks.list {
        println!("#{}", lists[&list.id].name);
        if let Some(v) = list.taskseries {
            for ts in v {
                println!("  {}", ts.name);
                for task in ts.task {
                    println!("    Due {:?}", task.due);
                }
            }
        }
    }
    Ok(())
}

async fn list_lists() -> Result<(), failure::Error> {
    let api = get_rtm_api().await?;
    let all_lists = api.get_lists().await?;
    for list in all_lists {
        println!("{}", list.name);
    }
    Ok(())
}

async fn add_tag(filter: String, tag: String) -> Result<(), failure::Error> {
    let api = get_rtm_api().await?;
    let timeline = api.get_timeline().await?;
    let tasks = api.get_tasks_filtered(&filter).await?;

    for list in tasks.list {
        if let Some(ref v) = list.taskseries {
            for ts in v {
                let to_tag = !ts.tags.contains(&tag);
                if to_tag {
                    println!("  Adding tag to {}...", ts.name);
                    api.add_tag(&timeline, &list, &ts, &ts.task[0], &[&tag[..]])
                        .await?;
                }
            }
        }
    }
    Ok(())
}

async fn add_task(name: String) -> Result<(), failure::Error> {
    let api = get_rtm_api().await?;
    let timeline = api.get_timeline().await?;

    api.add_task(&timeline, &name, None, None, None).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let opt = Opt::from_args();
    match opt.cmd {
        Command::Tasks { filter } => list_tasks(filter).await?,
        Command::Lists => list_lists().await?,
        Command::AddTag { filter, tag } => add_tag(filter, tag).await?,
        Command::AddTask { name } => add_task(name).await?,
        Command::AuthApp { key, secret } => auth_app(key, secret).await?,
        Command::Logout => logout().await?,
    }

    Ok(())
}
