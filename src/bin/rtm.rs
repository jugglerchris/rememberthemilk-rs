#![deny(warnings)]
use failure::bail;
use rememberthemilk::{Perms, API};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::io::Write;
use structopt::StructOpt;

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
    AuthApp {
        key: String,
        secret: String,
        #[structopt(default_value = "read", long)]
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

#[derive(StructOpt, Debug)]
struct Opt {
    #[structopt(short, long)]
    verbose: bool,

    #[structopt(short, long)]
    smart: bool,

    #[structopt(default_value = "auto", long)]
    colour: ColourOption,

    #[structopt(subcommand)]
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

async fn get_rtm_api(perm: Perms) -> Result<API, failure::Error> {
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

async fn auth_user(api: &mut API, perm: Perms) -> Result<(), failure::Error> {
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

async fn auth_app(key: String, secret: String, perm: Perms) -> Result<(), failure::Error> {
    let mut api = API::new(key, secret);

    auth_user(&mut api, perm).await?;
    println!("Successfully authenticated.");
    Ok(())
}

async fn logout() -> Result<(), failure::Error> {
    let mut config: rememberthemilk::RTMConfig = confy::load(RTM_APP_NAME, Some(RTM_AUTH_ID))?;
    config.clear_user_data();
    confy::store(RTM_APP_NAME, Some(RTM_AUTH_ID), config)?;
    Ok(())
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

async fn list_tasks(opts: &Opt, filter: &Option<String>) -> Result<(), failure::Error> {
    let api = get_rtm_api(Perms::Read).await?;
    let settings: Settings = confy::load(RTM_APP_NAME, RTM_SETTINGS)?;
    let filter = match filter {
        Some(ref s) => &s[..],
        None => &settings.filter,
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
    Ok(())
}

async fn list_lists() -> Result<(), failure::Error> {
    let api = get_rtm_api(Perms::Read).await?;
    let all_lists = api.get_lists().await?;
    for list in all_lists {
        println!("{}", list.name);
    }
    Ok(())
}

async fn add_tag(filter: String, tag: String) -> Result<(), failure::Error> {
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
    Ok(())
}

async fn add_task(opt: &Opt, name: &str) -> Result<(), failure::Error> {
    let api = get_rtm_api(Perms::Write).await?;
    let timeline = api.get_timeline().await?;

    api.add_task(&timeline, &name, None, None, None, opt.smart).await?;
    Ok(())
}

#[cfg(feature = "tui")]
mod tui {
    use tui::{
        backend::CrosstermBackend,
        widgets::{List, Block, Borders, BorderType, ListItem, ListState},
        Terminal, style::{Style, Color, Modifier}
    };
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use std::{time::Duration, io};

    pub async fn tui() -> Result<(), failure::Error> {
        enable_raw_mode()?;
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        terminal.draw(|f| {
            let size = f.size();
            let block = Block::default()
                .title("RTM list")
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(Color::White))
                .border_type(BorderType::Rounded)
                .style(Style::default().bg(Color::Black));
            let items = [
                ListItem::new("one"),
                ListItem::new("two"),
                ListItem::new("three"),
            ];
            let list = List::new(items)
                .block(block)
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol("*");
            let mut state: ListState = Default::default();
            state.select(Some(1));
            f.render_stateful_widget(list, size, &mut state);
        })?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        disable_raw_mode()?;
        terminal.show_cursor()?;

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    env_logger::init();

    let opt = Opt::from_args();
    match opt.cmd {
        Command::Tasks { ref filter } => list_tasks(&opt, filter).await?,
        Command::Lists => list_lists().await?,
        Command::AddTag { filter, tag } => add_tag(filter, tag).await?,
        Command::AddTask { ref name } => add_task(&opt, &name).await?,
        Command::AuthApp { key, secret, perm } => auth_app(key, secret, perm).await?,
        #[cfg(feature = "tui")]
        Command::Tui => tui::tui().await?,
        Command::Logout => logout().await?,
    }

    Ok(())
}
