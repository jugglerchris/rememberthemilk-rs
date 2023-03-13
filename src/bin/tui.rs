use chrono::{DateTime, Utc};
use rememberthemilk::{Perms, API, RTMTasks, RTMList, TaskSeries};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_stream::StreamExt;
use tui::{
    backend::CrosstermBackend,
    widgets::{List, Block, Borders, BorderType, ListItem, ListState, Paragraph, Clear},
    Terminal, style::{Style, Color, Modifier}, text::{Spans, Span}, layout::Rect
};
use crossterm::{terminal::{disable_raw_mode, enable_raw_mode}, event::{KeyCode, Event}};
use std::{io, borrow::Cow};

use crate::{get_rtm_api, get_default_filter, tail_end};

#[derive(Copy, Clone)]
enum DisplayMode {
    Tasks,
    Lists,
}

impl DisplayMode {
    fn title(&self) -> Cow<'static, str> {
        match self {
            DisplayMode::Tasks => "RTM Tasks".into(),
            DisplayMode::Lists => "RTM Lists".into(),
        }
    }
}

struct ListDispState {
    list: RTMList,
    opened: bool,
    tasks: Option<RTMTasks>,
}

struct UiState {
    display_mode: DisplayMode,
    filter: String,
    list_state: ListState,
    list_pos: usize,
    list_items: Vec<ListItem<'static>>,
    list_paths: Vec<(usize, usize)>,
    tasks: RTMTasks,
    lists: Vec<ListDispState>,
    lists_loading: bool,
    show_task: bool,
    input_prompt: &'static str,
    input_value: String,
    show_input: bool,
    event_tx: Sender<TuiEvent>,
}

struct RtmTaskListIterator<'t> {
    tasks: &'t RTMTasks,
    next_list_idx: Option<usize>,
    next_task_idx: usize,
}

impl<'t> RtmTaskListIterator<'t> {
    pub fn new(tasks: &'t RTMTasks) -> Self {
        RtmTaskListIterator {
            tasks,
            next_list_idx: Some(0),
            next_task_idx: 0,
        }
    }
}

impl<'t> Iterator for RtmTaskListIterator<'t> {
    type Item = &'t TaskSeries;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let list_idx = match self.next_list_idx {
                Some(idx) => idx,
                None => { return None; }
            };
            if list_idx >= self.tasks.list.len() {
                // We're done.
                self.next_list_idx = None;
                return None
            }
            let list = &self.tasks.list[list_idx];
            if let Some(vseries) = list.taskseries.as_ref() {
                if self.next_task_idx >= vseries.len() {
                    // Try the next list
                    self.next_list_idx = Some(list_idx + 1);
                    self.next_task_idx = 0;
                    continue;
                }
                let idx = self.next_task_idx;
                self.next_task_idx += 1;
                return Some(&vseries[idx])
            } else {
                // No taskseries
                self.next_list_idx  = Some(list_idx + 1);
                self.next_task_idx = 0;
                continue;
            }
        }
    }
}

enum TuiEvent {
    Input(Result<crossterm::event::Event, std::io::Error>),
    StateChanged,
}

struct Tui {
    api: API,
    event_rx: Receiver<TuiEvent>,
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    ui_state: std::sync::Arc<std::sync::Mutex<UiState>>,
}
enum StepResult {
    Cont,
    End,
}
impl Tui {
    pub async fn new() -> Result<Tui, failure::Error> {
        enable_raw_mode()?;
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut events = crossterm::event::EventStream::new();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(10);

        {
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                event_tx.send(TuiEvent::StateChanged).await.map_err(|_|()).unwrap();
                while let Some(evt) = events.next().await {
                    event_tx.send(TuiEvent::Input(evt)).await.map_err(|_|()).unwrap();
                }
            });
        }

        let api = get_rtm_api(Perms::Read).await?;
        let list_state: ListState = Default::default();
        let filter = get_default_filter()?;
        let show_task = false;
        let display_mode = DisplayMode::Tasks;

        let ui_state = UiState {
            display_mode,
            filter,
            list_state,
            list_pos: 0,
            list_items: vec![],
            list_paths: vec![],
            tasks: Default::default(),
            lists: Default::default(),
            lists_loading: false,
            show_task,
            input_prompt: "",
            input_value: String::new(),
            show_input: false,
            event_tx,
        };

        let mut tui = Tui {
            api,
            event_rx,
            terminal,
            ui_state: std::sync::Arc::new(std::sync::Mutex::new(ui_state)),
        };
        tui.update_tasks().await?;

        Ok(tui)
    }
    async fn update_tasks(&mut self) -> Result<(), failure::Error> {
        let filter = self.ui_state.lock().unwrap().filter.clone();
        let tasks = self.api.get_tasks_filtered(&filter).await?;
        let list_pos = 0;

        let mut list_items = vec![];
        let mut list_paths = vec![];
        for (ti, ts) in RtmTaskListIterator::new(&tasks).enumerate() {
            list_paths.push((ti, 0));
            list_items.push(ListItem::new(ts.name.clone()));
        }
        if list_items.is_empty() {
            list_items.push(ListItem::new("[No tasks in current list]"));
        }
        {
            let mut ui_state = self.ui_state.lock().unwrap();
            ui_state.list_state.select(Some(list_pos));
            ui_state.tasks = tasks;
            ui_state.list_items = list_items;
            ui_state.list_paths = list_paths;
            ui_state.list_pos = list_pos;
            ui_state.display_mode = DisplayMode::Tasks;
        }
        Ok(())
    }

    async fn update_list_display(&mut self) -> Result<(), failure::Error> {
        let mut list_items = vec![];
        let mut list_paths = vec![];
        let mut ui_state = self.ui_state.lock().unwrap();
        for (i, list) in ui_state.lists.iter().enumerate() {
            if let Some(tasks) = list.tasks.as_ref() {
                let len: usize =
                   tasks
                    .list
                    .iter()
                    .map(|l| l.taskseries.as_ref().map(|ts| ts.len()).unwrap_or(0))
                    .sum();
                if len > 0 {
                    list_items.push(
                        ListItem::new(
                                format!("{} [{}]", &list.list.name, len)
                            ).style(Style::default().fg(Color::LightYellow)));
                } else {
                    list_items.push(
                        ListItem::new(
                            format!("{}", &list.list.name)
                            ).style(Style::default().fg(Color::DarkGray)));
                }
            } else {
                list_items.push(ListItem::new(list.list.name.clone())
                            .style(Style::default().fg(Color::White)));
            }
            list_paths.push((i, 0));
            if list.opened {
                if let Some(tasks) = list.tasks.as_ref() {
                    for (ti, task) in RtmTaskListIterator::new(tasks).enumerate() {
                        list_paths.push((i, ti+1));
                        list_items.push(ListItem::new(format!("  {}", task.name)));
                    }
                }
            }
        }
        if list_items.is_empty() {
            if ui_state.lists_loading {
                list_items.push(ListItem::new("Loading..."));
            } else {
                list_items.push(ListItem::new("[No lists]"));
            }
        }
        ui_state.list_items = list_items;
        ui_state.list_paths = list_paths;
        Ok(())
    }

    async fn fetch_lists(api: API, ui_state: std::sync::Arc<std::sync::Mutex<UiState>>) {
        let lists = api.get_lists().await.unwrap();
        let tx = ui_state.lock().unwrap().event_tx.clone();
        {
            let mut ui_state = ui_state.lock().unwrap();
            ui_state.lists = lists.into_iter().map(|l| ListDispState {
                list: l,
                opened: false,
                tasks: None,
            }).collect();
            ui_state.lists_loading = false;
        }
        tx.send(TuiEvent::StateChanged).await.map_err(|_|()).unwrap();

        // Now fetch each list
        let (filter, ids) = {
            let ui_state = ui_state.lock().unwrap();
            let mut ids = Vec::new();
            for (i, list_state) in ui_state.lists.iter().enumerate() {
                ids.push((i, list_state.list.id.clone()));
            }

            (ui_state.filter.clone(), ids)
        };
        for (idx, list_id) in ids {
            let tasks = get_tasks(&api, &filter, &list_id).await.unwrap();
            {
                let mut ui_state = ui_state.lock().unwrap();
                if ui_state.lists.len() > idx && ui_state.lists[idx].list.id == list_id {
                    ui_state.lists[idx].tasks = Some(tasks);

                } else {
                    // We're out of step - return
                    return;
                }
            }
            tx.send(TuiEvent::StateChanged).await.map_err(|_|()).unwrap();
        }
        ui_state.lock().unwrap().lists_loading = false;
    }

    async fn update_lists(&mut self) -> Result<(), failure::Error> {
        {
            let mut ui_state = self.ui_state.lock().unwrap();
            ui_state.display_mode = DisplayMode::Lists;
            ui_state.lists_loading = true;
            ui_state.list_state.select(Some(0));

            ui_state.list_pos = 0;
            ui_state.show_task = false;

        }
        tokio::spawn(Tui::fetch_lists(self.api.clone(), std::sync::Arc::clone(&self.ui_state)));
        self.update_list_display().await
    }

    async fn draw(&mut self) -> Result<(), failure::Error> {
        let mut ui_state = self.ui_state.lock().unwrap();
        self.terminal.draw(move |f| {
            let size = f.size();
            let block = Block::default()
                .title(ui_state.display_mode.title().into_owned())
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(Color::White))
                .border_type(BorderType::Rounded)
                .style(Style::default().bg(Color::Black));
            let list = List::new(&ui_state.list_items[..])
                .block(block)
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol("*");
            let mut list_size = size;
            if ui_state.show_task {
                list_size.height = list_size.height / 2;
            }
            f.render_stateful_widget(list, list_size, &mut ui_state.list_state);

            if ui_state.show_task && !ui_state.list_paths.is_empty() {
                let block = Block::default()
                    .title("Task")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::White))
                    .border_type(BorderType::Rounded)
                    .style(Style::default().bg(Color::Black));
                let (li, _) = ui_state.list_paths[ui_state.list_pos];
                let series = RtmTaskListIterator::new(&ui_state.tasks).nth(li).unwrap();
                let mut text = vec![
                    Spans::from(vec![
                        Span::raw(series.name.clone()),
                    ])];
                if !series.tags.is_empty() {
                    let mut spans = vec![
                        Span::raw("Tags: ")];
                    let tag_style = Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD);
                    for tag in &series.tags {
                        spans.push(Span::styled(tag.clone(), tag_style));
                        spans.push(" ".into());
                    }
                    text.push( Spans::from(spans));
                }
                if let Some(repeat) = &series.repeat {
                    let style = Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD);
                    let mut spans = vec![
                        Span::raw("Repeat: ")];
                    if repeat.every {
                        spans.push(Span::raw("every "));
                    } else {
                        spans.push(Span::raw("after "));
                    }
                    spans.push(
                        Span::styled(repeat.rule.clone(), style));
                    text.push( Spans::from(spans));
                }
                for task in &series.task {
                    fn add_date_field(text: &mut Vec<Spans>, heading: &'static str,
                                      value: &Option<DateTime<Utc>>,
                                      color: Color) {
                        if let Some(date) = value {
                            let style = Style::default()
                                .fg(color)
                                .add_modifier(Modifier::BOLD);
                            let mut spans = vec![
                                Span::raw(heading)];
                            spans.push(
                                Span::styled(format!("{}", date), style));
                            text.push( Spans::from(spans));
                        }
                    }
                    add_date_field(&mut text, "Due: ", &task.due, Color::Yellow);
                    add_date_field(&mut text, "Completed: ", &task.completed, Color::Magenta);
                    add_date_field(&mut text, "Deleted: ", &task.deleted, Color::Red);
                }
                let par = Paragraph::new(text)
                    .block(block);
                let area = Rect::new(
                    0, list_size.height,
                    size.width, size.height - list_size.height);
                f.render_widget(par, area);
            }
            if ui_state.show_input {
                let block = Block::default()
                    .title(ui_state.input_prompt)
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::White))
                    .style(Style::default().bg(Color::Black));
                let area = Rect::new(0, size.height-2, size.width, 2);
                f.render_widget(Clear, area);

                let visible_value = tail_end(&ui_state.input_value, size.width as usize -1);
                let text = vec![
                    Span::raw(visible_value),
                    Span::raw("_"),
                ];
                f.render_widget(
                    Paragraph::new(vec![Spans::from(text)])
                        .block(block), area);
            }
        })?;
        Ok(())
    }

    async fn input(&mut self, prompt: &'static str) -> Result<String, failure::Error> {
        {
            let mut ui_state = self.ui_state.lock().unwrap();
            ui_state.input_value = ui_state.filter.clone();
            ui_state.input_prompt = prompt;
            ui_state.show_input = true;
        }
        loop {
            self.draw().await?;
            match self.event_rx.recv().await {
                None => { return Ok("".into()); }
                Some(TuiEvent::Input(ev)) => match ev {
                    Err(e) => { return Err(e.into()); }
                    Ok(ev) => match ev {
                        Event::Key(key) => {
                            match key.code {
                                KeyCode::Char(c) => {
                                    self.ui_state.lock().unwrap().input_value.push(c);
                                }
                                KeyCode::Enter => {
                                    break;
                                }
                                KeyCode::Backspace => {
                                    let _ = self.ui_state.lock().unwrap().input_value.pop();
                                }
                                KeyCode::Esc => {
                                    self.ui_state.lock().unwrap().show_input = false;
                                    return Ok(String::new());
                                }
                                _ => (),
                            }
                        }
                        _ => ()
                    }
                }
                Some(TuiEvent::StateChanged) => (),
            }
        }
        let mut ui_state = self.ui_state.lock().unwrap();
        ui_state.show_input = false;

        let mut result = String::new();
        std::mem::swap(&mut result, &mut ui_state.input_value);
        Ok(result)
    }

    pub async fn step(&mut self) -> Result<StepResult, failure::Error> {
        self.draw().await?;

        let result = match self.event_rx.recv().await {
            None => { return Ok(StepResult::End); }
            Some(TuiEvent::Input(ev)) => match ev {
                Err(e) => { return Err(e.into()); }
                Ok(ev) => match ev {
                    Event::Key(key) => {
                        match key.code {
                            KeyCode::Char('q') => {
                                StepResult::End
                            }
                            KeyCode::Char('g') => {
                                let filter = self.input("Enter RTM filter:").await?;
                                if !filter.is_empty() {
                                    self.ui_state.lock().unwrap().filter = filter;
                                    self.update_tasks().await?;
                                }
                                StepResult::Cont
                            }
                            KeyCode::Char('L') => {
                                self.update_lists().await?;
                                StepResult::Cont
                            }
                            KeyCode::Enter => {
                                let mut ui_state = self.ui_state.lock().unwrap();
                                match ui_state.display_mode {
                                    DisplayMode::Tasks => {
                                        ui_state.show_task = !ui_state.show_task;
                                    }
                                    DisplayMode::Lists => {
                                        // Expand/unexpand the list
                                        if ui_state.list_items.len() > 0 {
                                            let (li, ti) = ui_state.list_paths[ui_state.list_pos];
                                            if ti == 0 {
                                                let new_opened = !ui_state.lists[li].opened;
                                                ui_state.lists[li].opened = new_opened;
                                            }
                                        }
                                        drop(ui_state);
                                        self.update_list_display().await?;
                                    }
                                }
                                StepResult::Cont
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                let mut ui_state = self.ui_state.lock().unwrap();
                                ui_state.list_pos = ui_state.list_pos.saturating_sub(1);
                                let list_pos = ui_state.list_pos;
                                ui_state.list_state.select(Some(list_pos));
                                StepResult::Cont
                            }
                            KeyCode::Down | KeyCode::Char('j')  => {
                                let mut ui_state = self.ui_state.lock().unwrap();
                                if ui_state.list_pos+1 < ui_state.list_items.len() {
                                    ui_state.list_pos += 1;
                                }
                                let list_pos = ui_state.list_pos;
                                ui_state.list_state.select(Some(list_pos));
                                StepResult::Cont
                            }
                            _ => StepResult::Cont,
                        }
                    }
                    _ => StepResult::Cont,
                }
            }
            Some(TuiEvent::StateChanged) => {
                let display_mode = self.ui_state.lock().unwrap().display_mode;
                match display_mode {
                    DisplayMode::Tasks => {}
                    DisplayMode::Lists => {
                        self.update_list_display().await?;
                    }
                }
                StepResult::Cont
            }
        };
        Ok(result)
    }

    pub async fn run(&mut self) -> Result<(), failure::Error> {
        loop {
            match self.step().await? {
                StepResult::End => break,
                StepResult::Cont => (), // continue
            }
        }
        Ok(())
    }
}

async fn get_tasks(api: &rememberthemilk::API, filter: &str, id: &str) -> Result<RTMTasks, failure::Error> {
    let tasks = api.get_tasks_in_list(id, filter).await?;
    Ok(tasks)
}

impl Drop for Tui {
    fn drop(&mut self) {
        disable_raw_mode().unwrap();
        self.terminal.show_cursor().unwrap();
    }
}

pub async fn tui() -> Result<(), failure::Error> {
    let mut tui = Tui::new().await?;

    tui.run().await?;

    Ok(())
}

