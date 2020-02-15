use failure::bail;
use structopt::StructOpt;
use confy;
use rememberthemilk::API;
use std::{env, collections::HashMap};

#[derive(StructOpt,Debug)]
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
    }
}

#[derive(StructOpt,Debug)]
struct Opt {
    #[structopt(subcommand)]
    cmd: Command,
}

async fn get_rtm_api() -> Result<API, failure::Error>
{
    let config: rememberthemilk::RTMConfig = confy::load("rtm_auth_example")?;
    let mut api = if config.api_key.is_some() && config.api_secret.is_some() {
        let api = API::from_config(config);
        api
    } else {
        let args: Vec<String> = env::args().collect();
        let api_key = args[1].clone();
        let api_secret = args[2].clone();

        let api = API::new(api_key, api_secret);
        api
    };

    if !api.has_token().await.unwrap() {
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
    };
    Ok(api)
}

async fn list_tasks(filter: Option<String>) -> Result<(), failure::Error>
{
    let api = get_rtm_api().await?;
    let filter = match filter {
        Some(ref s) => &s[..],
        None => "status:incomplete AND (dueBefore:today OR due:today)",
    };
    let all_tasks = api.get_tasks_filtered(filter).await?;
    let mut lists = HashMap::new();
    if all_tasks.list.len() > 0 {
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

async fn list_lists() -> Result<(), failure::Error>
{
    let api = get_rtm_api().await?;
    let all_lists = api.get_lists().await?;
    for list in all_lists {
        println!("{}", list.name);
    }
    Ok(())
}

async fn add_tag(filter: String, tag: String) -> Result<(), failure::Error>
{
    let api = get_rtm_api().await?;
    let timeline = api.get_timeline().await?;
    let tasks = api.get_tasks_filtered(&filter).await?;

    for list in tasks.list {
        if let Some(ref v) = list.taskseries {
            for ts in v {
                let to_tag = !ts.tags.contains(&tag);
                if to_tag {
                    println!("  Adding tag to {}...", ts.name);
                    api.add_tag(&timeline, &list, &ts, &ts.task[0], &[&tag[..]]).await?;
                }
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), failure::Error>
{
    let opt = Opt::from_args();
    match opt.cmd {
        Command::Tasks { filter } => {
            list_tasks(filter).await?
        }
        Command::Lists => {
            list_lists().await?
        }
        Command::AddTag { filter, tag } => {
            add_tag(filter, tag).await?
        }
    }

    Ok(())
}
