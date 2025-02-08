#![deny(warnings)]
use anyhow::bail;
use rememberthemilk::{Perms, API};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;
use clap::Parser;

const RTM_APP_NAME: &'static str = "rtm";
const RTM_AUTH_ID: &'static str = "rtm_auth";
const RTM_SETTINGS: &'static str = "config";

#[derive(Serialize, Deserialize)]
/// rtm tool user configuration.
/// This is intended to be user-editable.
pub struct Settings {
    /// The default search filter for `rtm tasks` when not otherwise
    /// specified.
    pub filter: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            filter: "status:incomplete AND (dueBefore:today OR due:today)".into(),
        }
    }
}

fn tail_end(input: &str, width: usize) -> String {
    let tot_width = unicode_width::UnicodeWidthStr::width(input);
    if tot_width <= width {
        // It fits, no problem.
        return input.into();
    }
    // Otherwise, trim off the start, making space for a ...
    let mut result = "…".to_string();
    let elipsis_width = unicode_width::UnicodeWidthStr::width(result.as_str());
    let space_needed = tot_width - (width - elipsis_width);

    let mut removed_space = 0;
    let mut ci = input.char_indices();

    for (_, c) in &mut ci {
        if let Some(w) = unicode_width::UnicodeWidthChar::width(c) {
            removed_space += w;
            if removed_space >= space_needed {
                break;
            }
        }
    }
    let (start, _) = ci.next().unwrap();
    result.push_str(&input[start..]);
    result
}

#[derive(Parser, Debug)]
enum Command {
    /// Operate on tasks
    Tasks {
        #[clap(long)]
        /// Provide a filter string in RTM format.
        filter: Option<String>,

        #[clap(long)]
        /// Look only for items with the given external id.
        extid: Option<String>,
    },
    /// Show all lists
    Lists,
    /// Add a tag to filtered messages
    AddTag {
        tag: String,
        #[clap(long)]
        filter: String,
    },
    /// Add a new task
    AddTask {
        name: String,
        #[clap(long)]
        external_id: Option<String>,
    },
    /// Authorise the app
    AuthApp {
        key: String,
        secret: String,
        #[clap(default_value = "read", long)]
        perm: Perms,
    },
    #[cfg(feature = "tui")]
    /// Run the TUI
    Tui,
    /// Remove the saved user token
    Logout,
}

#[derive(Copy, Clone, Debug)]
enum ColourOption {
    Auto,
    Always,
    Never,
}

impl std::str::FromStr for ColourOption {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<ColourOption, &'static str> {
        match s {
            "auto" => Ok(ColourOption::Auto),
            "always" => Ok(ColourOption::Always),
            "never" => Ok(ColourOption::Never),
            _ => Err("Invalid option for --colour"),
        }
    }
}

#[derive(Parser, Debug)]
struct Opt {
    #[clap(short, long)]
    verbose: bool,

    #[clap(short, long)]
    smart: bool,

    #[clap(default_value = "auto", long)]
    colour: ColourOption,

    #[clap(subcommand)]
    cmd: Command,
}

impl Opt {
    fn get_stdout(&self) -> termcolor::StandardStream {
        use termcolor::ColorChoice;
        let choice = match self.colour {
            ColourOption::Auto => ColorChoice::Auto,
            ColourOption::Always => ColorChoice::Always,
            ColourOption::Never => ColorChoice::Never,
        };
        termcolor::StandardStream::stdout(choice)
    }
}

async fn get_rtm_api(perm: Perms) -> Result<API, anyhow::Error> {
    let config: rememberthemilk::RTMConfig = confy::load(RTM_APP_NAME, Some(RTM_AUTH_ID))?;
    let mut api = if config.api_key.is_some() && config.api_secret.is_some() {
        API::from_config(config)
    } else {
        eprintln!("Error, no API key saved.  Use `rtm auth-app` to supply them.");
        bail!("No auth key");
    };

    if !api.has_token(perm).await.unwrap() {
        println!("We don't have the correct permissions - trying to authenticate.");
        auth_user(&mut api, perm).await?;
    };
    Ok(api)
}

async fn auth_user(api: &mut API, perm: Perms) -> Result<(), anyhow::Error> {
    let auth = api.start_auth(perm).await?;
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
    confy::store(RTM_APP_NAME, Some(RTM_AUTH_ID), api.to_config())?;
    Ok(())
}

async fn auth_app(key: String, secret: String, perm: Perms) -> Result<ExitCode, anyhow::Error> {
    let mut api = API::new(key, secret);

    auth_user(&mut api, perm).await?;
    println!("Successfully authenticated.");
    Ok(ExitCode::SUCCESS)
}

async fn logout() -> Result<ExitCode, anyhow::Error> {
    let mut config: rememberthemilk::RTMConfig = confy::load(RTM_APP_NAME, Some(RTM_AUTH_ID))?;
    config.clear_user_data();
    confy::store(RTM_APP_NAME, Some(RTM_AUTH_ID), config)?;
    Ok(ExitCode::SUCCESS)
}

fn format_human_time(secs: u64) -> String {
    if secs > 24 * 60 * 60 {
        let days = secs / (24 * 60 * 60);
        format!("{} day{}", days, if days > 1 { "s" } else { "" })
    } else if secs > 60 * 60 {
        let hours = secs / (60 * 60);
        format!("{} hour{}", hours, if hours > 1 { "s" } else { "" })
    } else if secs > 60 {
        let minutes = secs / 60;
        format!("{} minute{}", minutes, if minutes > 1 { "s" } else { "" })
    } else {
        format!("{} sec{}", secs, if secs > 1 { "s" } else { "" })
    }
}

fn get_default_filter() -> Result<String, anyhow::Error> {
    let settings: Settings = confy::load(RTM_APP_NAME, RTM_SETTINGS)?;
    Ok(settings.filter)
}

async fn list_tasks(
    opts: &Opt,
    filter: &Option<String>,
    extid: &Option<String>,
) -> Result<ExitCode, anyhow::Error> {
    let api = get_rtm_api(Perms::Read).await?;
    let default_filter = get_default_filter()?;
    let extid_filter;
    let filter = match (filter, extid) {
        (Some(ref s), None) => &s[..],
        (None, Some(ref s)) => {
            extid_filter = api.get_filter_extid(s);
            &extid_filter[..]
        }
        (Some(_), Some(_)) => {
            bail!("Supplying both --filter and --extid is not supported.")
        }
        (None, None) => &default_filter,
    };
    let all_tasks = api.get_tasks_filtered(filter).await?;
    let mut lists = HashMap::new();
    if !all_tasks.list.is_empty() {
        let all_lists = api.get_lists().await?;
        for list in all_lists {
            lists.insert(list.id.clone(), list);
        }
    }
    use termcolor::{Color, ColorSpec, WriteColor};
    if all_tasks.list.is_empty() {
        return Ok(ExitCode::from(1));
    }
    let mut stdout = opts.get_stdout();
    for list in all_tasks.list {
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Magenta)))?;
        writeln!(stdout, "#{}", lists[&list.id].name)?;
        if let Some(v) = list.taskseries {
            stdout.reset()?;
            for ts in v {
                log::trace!("{:?}", ts.task);
                for task in &ts.task {
                    let time_left = task.get_time_left();
                    use rememberthemilk::TimeLeft::*;
                    match time_left {
                        Remaining(secs) => {
                            let colour = if secs < 60 * 60 {
                                ColorSpec::new().set_fg(Some(Color::Red)).clone()
                            } else {
                                ColorSpec::new().set_fg(Some(Color::Yellow)).clone()
                            };
                            stdout.set_color(&colour)?;
                            write!(stdout, "{}", format_human_time(secs))?;
                        }
                        Overdue(secs) => {
                            stdout.set_color(ColorSpec::new().set_bg(Some(Color::Red)))?;
                            write!(stdout, "{} ago", format_human_time(secs))?;
                        }
                        Completed | NoDue => {
                            ColorSpec::new().set_fg(Some(Color::Green));
                        }
                    };
                }
                write!(stdout, "  {}", ts.name)?;
                stdout.set_color(ColorSpec::new().set_bg(Some(Color::Black)))?;
                writeln!(stdout, "")?;
                if opts.verbose {
                    writeln!(stdout, "   id: {}", ts.id)?;
                    writeln!(stdout, "   created: {}", ts.created)?;
                    writeln!(stdout, "   modified: {}", ts.modified)?;
                    writeln!(stdout, "   tags: {:?}", &ts.tags[..])?;
                    if let Some(repeat) = ts.repeat {
                        if repeat.every {
                            writeln!(stdout, "   repeat: every {}", repeat.rule)?;
                        } else {
                            writeln!(stdout, "   repeat: after {}", repeat.rule)?;
                        }
                    }
                }

                if opts.verbose && !ts.task.is_empty() {
                    let task = &ts.task[0];
                    writeln!(stdout, "    id: {}", task.id)?;
                    if let Some(due) = task.due {
                        if task.has_due_time {
                            writeln!(stdout, "    due: {}", due)?;
                        } else {
                            // Remove the time parts, which aren't used.
                            writeln!(stdout, "    due: {}", due.date_naive())?;
                        }
                    }
                    if let Some(added) = task.added {
                        writeln!(stdout, "    added: {}", added)?;
                    }
                    if let Some(completed) = task.completed {
                        writeln!(stdout, "    completed: {}", completed)?;
                    }
                    if let Some(deleted) = task.deleted {
                        writeln!(stdout, "    deleted: {}", deleted)?;
                    }
                }
            }
        }
    }
    stdout.reset()?;
    Ok(ExitCode::SUCCESS)
}

async fn list_lists() -> Result<ExitCode, anyhow::Error> {
    let api = get_rtm_api(Perms::Read).await?;
    let all_lists = api.get_lists().await?;
    for list in all_lists {
        println!("{}", list.name);
    }
    Ok(ExitCode::SUCCESS)
}

async fn add_tag(filter: String, tag: String) -> Result<ExitCode, anyhow::Error> {
    let api = get_rtm_api(Perms::Write).await?;
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
    Ok(ExitCode::SUCCESS)
}

async fn add_task(
    opt: &Opt,
    name: &str,
    external_id: Option<&str>,
) -> Result<ExitCode, anyhow::Error> {
    let api = get_rtm_api(Perms::Write).await?;
    let timeline = api.get_timeline().await?;

    let added = api
        .add_task(&timeline, &name, None, None, external_id, opt.smart)
        .await?;
    if let Some(list) = added {
        if let Some(taskseries) = list.taskseries {
            if taskseries.len() > 0 {
                print_taskseries(&taskseries[0]);
            } else {
                println!("Successful result, but no task in series.")
            }
        } else {
            println!("Successful result, but no task series.")
        }
    } else {
        println!("Successful result, but no list returned.")
    }
    Ok(ExitCode::SUCCESS)
}

fn print_taskseries(task: &rememberthemilk::TaskSeries) {
    println!("Added task id {}", task.id);
    println!("Name: {}", task.name);
    println!("Tags: {}", task.tags.join(", "));
    for task in &task.task {
        if task.completed.is_none() {
            println!("  Due: {:?}", task.due);
        }
    }
}

#[cfg(feature = "tui")]
mod tui;

#[tokio::main]
async fn main() -> Result<ExitCode, anyhow::Error> {
    env_logger::init();

    let opt = Opt::parse();
    Ok(match opt.cmd {
        Command::Tasks {
            ref filter,
            ref extid,
        } => list_tasks(&opt, filter, extid).await?,
        Command::Lists => list_lists().await?,
        Command::AddTag { filter, tag } => add_tag(filter, tag).await?,
        Command::AddTask {
            ref name,
            ref external_id,
        } => add_task(&opt, &name, external_id.as_deref()).await?,
        Command::AuthApp { key, secret, perm } => auth_app(key, secret, perm).await?,
        #[cfg(feature = "tui")]
        Command::Tui => tui::tui().await?,
        Command::Logout => logout().await?,
    })
}
